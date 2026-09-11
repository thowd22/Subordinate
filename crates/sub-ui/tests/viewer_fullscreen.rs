//! The fullscreen monitor picker, on the shared `egui_kittest` harness.
//!
//! What a panel can be driven for, TASK-68 asks to be driven: the picker's
//! rows are clicked, the toggle is clicked, and the assertions are on the
//! model — which display was chosen, what the pop-out window is then asked
//! for, and whether Escape put the review picture back in a window.
//!
//! The fullscreen presentation itself — a borderless window filling a real
//! second head on Wayland, X11, Windows and macOS — is TASK-118's job on
//! hardware; nothing here creates a window.

mod support;

use eframe::egui;
use egui_kittest::kittest::Queryable;
use sub_ui::fullscreen::{
    CURRENT_DISPLAY_LABEL, ENTER_LABEL, FullscreenAction, FullscreenState, LEAVE_LABEL, Monitor,
    MonitorList, monitor_picker_ui,
};
use sub_ui::popout::{PopoutViewer, popout_viewport_builder, popout_viewport_ui};

/// The displays a review setup has: the editing screen and the TV.
fn two_displays() -> MonitorList {
    MonitorList::new(vec![
        Monitor::new("eDP-1", [1920.0, 1080.0], true),
        Monitor::new("HDMI-1", [3840.0, 2160.0], false),
    ])
}

/// What the picker's tests drive: the choice, the pop-out it applies to, and
/// the last action the picker returned.
struct Scene {
    /// The chosen display, as the View menu's picker leaves it.
    fullscreen: FullscreenState,
    /// The pop-out window the choice is applied to.
    popout: PopoutViewer,
    /// The last thing the picker asked for, kept across frames: `Harness::run`
    /// runs egui until it settles, so the frame after the click asks for
    /// nothing and would otherwise wipe the answer.
    action: Option<FullscreenAction>,
}

impl Scene {
    /// A scene with both displays listed and nothing chosen.
    fn new() -> Self {
        let mut fullscreen = FullscreenState::new();
        fullscreen.set_monitors(two_displays());
        Self {
            fullscreen,
            popout: PopoutViewer::new(),
            action: None,
        }
    }

    /// One frame of the picker, applying what it asked for exactly as
    /// `SubordinateApp` does.
    fn frame(&mut self, ui: &mut egui::Ui) {
        let fullscreen = self.popout.is_fullscreen();
        let Some(action) = monitor_picker_ui(ui, &mut self.fullscreen, fullscreen) else {
            return;
        };
        self.action = Some(action);
        match action {
            FullscreenAction::Choose(_) => {
                self.popout.set_monitor(self.fullscreen.monitor_index());
            }
            FullscreenAction::Enter => {
                self.popout.set_monitor(self.fullscreen.monitor_index());
                self.popout.set_fullscreen(true);
            }
            FullscreenAction::Leave => self.popout.set_fullscreen(false),
        }
    }
}

#[test]
fn the_picker_lists_every_display_and_records_the_one_clicked() {
    let mut harness =
        support::panel_harness_state(Scene::new(), |ui, scene: &mut Scene| scene.frame(ui));
    harness.run();

    // Every display the editor knows about has a row, named the way the user
    // counts displays.
    harness.get_by_label(CURRENT_DISPLAY_LABEL);
    harness.get_by_label("1: eDP-1 - 1920x1080 (primary)");
    harness.get_by_label("2: HDMI-1 - 3840x2160");
    assert_eq!(
        harness.state().fullscreen.monitor_index(),
        None,
        "nothing is chosen until a row is clicked"
    );

    harness.get_by_label("2: HDMI-1 - 3840x2160").click();
    harness.run();
    let scene = harness.state();
    assert_eq!(
        scene.action,
        Some(FullscreenAction::Choose(Some(1))),
        "the picker hands the choice back to the application"
    );
    assert_eq!(scene.fullscreen.monitor_index(), Some(1));
    assert_eq!(
        scene.fullscreen.settings().monitor_name(),
        Some("HDMI-1"),
        "and remembers the display by name, not only by slot"
    );
    assert_eq!(
        scene.popout.monitor(),
        Some(1),
        "the pop-out window is told which display to take over"
    );

    harness.get_by_label(CURRENT_DISPLAY_LABEL).click();
    harness.run();
    assert_eq!(
        harness.state().fullscreen.monitor_index(),
        None,
        "and the current display puts the choice back"
    );
}

#[test]
fn the_toggle_takes_the_viewer_fullscreen_on_the_chosen_display() {
    let mut harness =
        support::panel_harness_state(Scene::new(), |ui, scene: &mut Scene| scene.frame(ui));
    harness.run();
    assert!(
        !harness.state().popout.is_fullscreen(),
        "the viewer starts as a window"
    );

    harness.get_by_label("2: HDMI-1 - 3840x2160").click();
    harness.run();
    harness.get_by_label(ENTER_LABEL).click();
    harness.run();

    let scene = harness.state();
    assert!(scene.popout.is_fullscreen(), "the toggle went fullscreen");
    assert!(
        scene.popout.is_open(),
        "which opens the pop-out window there was nothing else to fill"
    );
    let builder = popout_viewport_builder(scene.popout.placement());
    assert_eq!(builder.fullscreen, Some(true));
    assert_eq!(
        builder.monitor,
        Some(1),
        "and the window eframe is asked for is the one on the TV"
    );

    // The same item is both directions, so it now offers the way out.
    harness.get_by_label(LEAVE_LABEL).click();
    harness.run();
    let scene = harness.state();
    assert!(!scene.popout.is_fullscreen(), "and it comes back");
    assert_eq!(
        popout_viewport_builder(scene.popout.placement()).fullscreen,
        Some(false)
    );
    assert!(
        scene.popout.is_open(),
        "leaving fullscreen keeps the pop-out window"
    );
}

#[test]
fn escape_in_the_fullscreen_window_returns_it_to_a_window() {
    let mut popout = PopoutViewer::new();
    popout.set_monitor(Some(1));
    popout.set_fullscreen(true);

    // One pass of the fullscreen pop-out window, with Escape pressed in it.
    let mut harness = support::builder().build_ui({
        let shared = std::sync::Arc::clone(popout.shared());
        move |ui| popout_viewport_ui(ui, &shared)
    });
    harness.run();
    harness.key_press(egui::Key::Escape);
    harness.run();

    assert!(
        !popout.poll_fullscreen_exit(),
        "Escape reaches the main pass as a request to leave fullscreen"
    );
    assert!(
        popout.is_open(),
        "and leaves the viewer popped out rather than closing it"
    );
    assert_eq!(
        popout_viewport_builder(popout.placement()).fullscreen,
        Some(false),
        "the next window eframe is asked for is a window again"
    );
}
