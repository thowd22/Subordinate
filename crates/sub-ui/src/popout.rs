//! The pop-out viewer: the preview in an OS window of its own, on whichever
//! monitor the user drags it to (docs/PLAN.md §2, §5.7).
//!
//! The pop-out is an egui *deferred viewport*: eframe gives it a real window
//! and calls its paint closure once a frame, on the same `wgpu` device egui
//! and [`sub_render::Compositor`] already share. That is what makes it free:
//! the closure paints the very [`egui::TextureId`] the docked viewer paints,
//! so there is no second composite, no readback and no copy — only one more
//! quad sampling a texture that already exists.
//!
//! eframe keeps the closure across frames, so it must be `Send + Sync +
//! 'static` and cannot borrow the app. Everything it needs therefore crosses
//! through [`PopoutShared`]: the main pass publishes the picture into it, and
//! the closure hands back what happened in its window — the playback keys it
//! saw, and whether the user closed it. The main pass is the only thing that
//! touches the playhead, so the pop-out never mutates the model itself.
//!
//! ```
//! use sub_ui::popout::PopoutViewer;
//! use sub_ui::viewer::ViewerFrame;
//! use eframe::egui;
//!
//! let mut popout = PopoutViewer::new();
//! assert!(!popout.is_open());
//!
//! // The Viewer menu item opens it; the picture is published, not copied.
//! popout.toggle();
//! popout.publish(ViewerFrame::new(egui::TextureId::User(1), 1920, 1080));
//! assert!(popout.is_open());
//! assert_eq!(popout.shared().frame().map(|frame| frame.width), Some(1920));
//!
//! // Closing its window puts the viewer back in the dock.
//! popout.shared().request_close();
//! assert!(!popout.poll_close());
//! assert!(!popout.is_open());
//! ```

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::shortcuts::{Action, ShortcutMap};
use crate::viewer::{TransportAction, ViewerAction, ViewerFrame, paint_picture};

/// The pop-out window's title.
pub const POPOUT_TITLE: &str = "Subordinate viewer";

/// The menu item that opens the pop-out.
pub const OPEN_LABEL: &str = "Pop out viewer";

/// The menu item that puts it back.
pub const CLOSE_LABEL: &str = "Return viewer to the dock";

/// The pop-out window's initial size, in points.
const POPOUT_SIZE: [f32; 2] = [960.0, 540.0];

/// The stable id of the pop-out viewport.
///
/// Deferred viewports are identified by this rather than by their window, so
/// it has to be the same value on every frame the pop-out is open.
#[must_use]
pub fn popout_viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("sub-ui.viewer.popout")
}

/// Whether `action` is one the pop-out answers.
///
/// The pop-out is a preview window: it takes the keys that move the playhead
/// and the keys that drive the transport, and leaves editing, saving and the
/// rest to the editor window that owns the project.
#[must_use]
pub fn is_playback_action(action: Action) -> bool {
    ViewerAction::for_action(action).is_some() || TransportAction::for_action(action).is_some()
}

/// The playback half of `map`: the bindings the pop-out claims.
///
/// Polling with the reduced map is what stops the pop-out from swallowing a
/// chord it would then throw away.
#[must_use]
pub fn playback_shortcuts(map: &ShortcutMap) -> ShortcutMap {
    let mut playback = ShortcutMap::empty();
    for binding in map.bindings() {
        if is_playback_action(binding.action) {
            playback.push(*binding);
        }
    }
    playback
}

/// What the pop-out's paint closure and the main pass pass between them.
///
/// One direction carries the picture: a [`ViewerFrame`] is a texture id and a
/// canvas size, so publishing it is a couple of words, never a frame of pixels.
/// The other carries what the user did in that window.
#[derive(Debug, Default)]
pub struct PopoutShared {
    /// The picture to paint, as published by the last main pass.
    frame: Mutex<Option<ViewerFrame>>,
    /// The bindings the pop-out window answers.
    shortcuts: Mutex<ShortcutMap>,
    /// Playback actions seen in the pop-out window, waiting for the main pass.
    actions: Mutex<Vec<Action>>,
    /// Set when the window manager or the user closed the pop-out.
    close_requested: AtomicBool,
    /// How many frames the pop-out has painted, for diagnostics and tests.
    frames_painted: AtomicU64,
}

impl PopoutShared {
    /// Shared state with no picture and no bindings yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            frame: Mutex::new(None),
            shortcuts: Mutex::new(ShortcutMap::empty()),
            actions: Mutex::new(Vec::new()),
            close_requested: AtomicBool::new(false),
            frames_painted: AtomicU64::new(0),
        }
    }

    /// Publishes the picture the pop-out should paint next.
    pub fn set_frame(&self, frame: ViewerFrame) {
        if let Ok(mut slot) = self.frame.lock() {
            *slot = Some(frame);
        }
    }

    /// Forgets the picture, so the pop-out paints black.
    pub fn clear_frame(&self) {
        if let Ok(mut slot) = self.frame.lock() {
            *slot = None;
        }
    }

    /// The picture last published, if there is one.
    #[must_use]
    pub fn frame(&self) -> Option<ViewerFrame> {
        self.frame.lock().ok().and_then(|slot| *slot)
    }

    /// Replaces the bindings the pop-out answers, if they changed.
    ///
    /// The map is compared before it is stored, so the common case — the
    /// keymap standing still for the whole session — allocates nothing.
    pub fn set_shortcuts(&self, map: &ShortcutMap) {
        if let Ok(mut stored) = self.shortcuts.lock()
            && *stored != *map
        {
            *stored = map.clone();
        }
    }

    /// The bindings the pop-out answers.
    #[must_use]
    pub fn shortcuts(&self) -> ShortcutMap {
        self.shortcuts
            .lock()
            .map_or_else(|_| ShortcutMap::empty(), |map| map.clone())
    }

    /// Queues a playback action the pop-out window saw.
    ///
    /// Anything else is dropped: the pop-out has no project to edit.
    pub fn push_action(&self, action: Action) {
        if !is_playback_action(action) {
            return;
        }
        if let Ok(mut queued) = self.actions.lock() {
            queued.push(action);
        }
    }

    /// Takes everything queued since the last call.
    #[must_use]
    pub fn take_actions(&self) -> Vec<Action> {
        self.actions
            .lock()
            .map_or_else(|_| Vec::new(), |mut queued| std::mem::take(&mut *queued))
    }

    /// Records that the pop-out window was closed.
    pub fn request_close(&self) {
        self.close_requested.store(true, Ordering::Relaxed);
    }

    /// Takes the close request, if one is pending.
    #[must_use]
    pub fn take_close_request(&self) -> bool {
        self.close_requested.swap(false, Ordering::Relaxed)
    }

    /// How many frames the pop-out has painted.
    #[must_use]
    pub fn frames_painted(&self) -> u64 {
        self.frames_painted.load(Ordering::Relaxed)
    }

    /// Counts one painted pop-out frame.
    fn count_frame(&self) {
        self.frames_painted.fetch_add(1, Ordering::Relaxed);
    }
}

/// The pop-out viewer: whether it is open, and the state its window shares
/// with the editor.
#[derive(Debug, Clone)]
pub struct PopoutViewer {
    /// Whether the pop-out window should exist this frame.
    open: bool,
    /// Where the window should be placed, in points on the virtual desktop.
    ///
    /// `None` leaves the placement to the window manager, which is what a
    /// user gets. A position is how the CI smoke run puts the pop-out on the
    /// second monitor without a human dragging it there.
    position: Option<[f32; 2]>,
    /// What the pop-out's paint closure reads and writes.
    shared: Arc<PopoutShared>,
}

impl Default for PopoutViewer {
    fn default() -> Self {
        Self::new()
    }
}

impl PopoutViewer {
    /// A closed pop-out.
    #[must_use]
    pub fn new() -> Self {
        Self {
            open: false,
            position: None,
            shared: Arc::new(PopoutShared::new()),
        }
    }

    /// Places the window at `position`, in points on the virtual desktop.
    ///
    /// It applies from the next paint onwards, so setting it before the
    /// pop-out is opened is what puts the first window in the right place.
    pub const fn set_position(&mut self, position: [f32; 2]) {
        self.position = Some(position);
    }

    /// Where the window is asked to appear, if anywhere.
    #[must_use]
    pub const fn position(&self) -> Option<[f32; 2]> {
        self.position
    }

    /// Whether the viewer is popped out.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Opens the pop-out. A pop-out that is already open is left alone.
    pub fn open(&mut self) {
        if !self.open {
            let _ = self.shared.take_close_request();
            self.open = true;
        }
    }

    /// Closes the pop-out, returning the viewer to the dock.
    pub fn close(&mut self) {
        self.open = false;
        let _ = self.shared.take_close_request();
        self.shared.clear_frame();
    }

    /// Opens the pop-out if it is closed and closes it if it is open.
    ///
    /// Returns whether it is open afterwards.
    pub fn toggle(&mut self) -> bool {
        if self.open {
            self.close();
        } else {
            self.open();
        }
        self.open
    }

    /// The state shared with the pop-out's paint closure.
    #[must_use]
    pub fn shared(&self) -> &Arc<PopoutShared> {
        &self.shared
    }

    /// Publishes the picture the pop-out should paint.
    ///
    /// This is the whole of "sharing the compositor texture": the same
    /// [`egui::TextureId`] the docked viewer paints is handed to the pop-out,
    /// so the picture crosses windows without being rendered twice.
    pub fn publish(&self, frame: ViewerFrame) {
        self.shared.set_frame(frame);
    }

    /// Takes the playback actions the pop-out window saw.
    #[must_use]
    pub fn take_actions(&self) -> Vec<Action> {
        self.shared.take_actions()
    }

    /// Applies a pending close request and reports whether the pop-out is
    /// still open.
    ///
    /// The close arrives on the pop-out's own paint pass, so it is picked up
    /// here, in the main pass, where the docked viewer can take the picture
    /// back on the same frame.
    pub fn poll_close(&mut self) -> bool {
        if self.shared.take_close_request() {
            self.close();
        }
        self.open
    }

    /// Draws the pop-out window, if it is open.
    ///
    /// `map` is the keyboard map in force; the pop-out answers the playback
    /// half of it. Returns whether the pop-out is open after any close its
    /// window reported.
    pub fn show(&mut self, ctx: &egui::Context, map: &ShortcutMap) -> bool {
        if !self.poll_close() {
            return false;
        }
        self.shared.set_shortcuts(&playback_shortcuts(map));
        let shared = Arc::clone(&self.shared);
        ctx.show_viewport_deferred(
            popout_viewport_id(),
            popout_viewport_builder(self.position),
            move |ui, _class| popout_viewport_ui(ui, &shared),
        );
        true
    }
}

/// The window the pop-out asks eframe for.
///
/// Split out of [`PopoutViewer::show`] so the placement can be asserted
/// without a display attached.
#[must_use]
pub fn popout_viewport_builder(position: Option<[f32; 2]>) -> egui::ViewportBuilder {
    let builder = egui::ViewportBuilder::default()
        .with_title(POPOUT_TITLE)
        .with_inner_size(POPOUT_SIZE);
    match position {
        Some(position) => builder.with_position(position),
        None => builder,
    }
}

/// One paint of the pop-out window.
///
/// Split out of the closure so it can be driven by a test pass: everything
/// the pop-out does — the picture, the keyboard and the close — happens here.
pub fn popout_viewport_ui(ui: &mut egui::Ui, shared: &PopoutShared) {
    let frame = shared.frame();
    let shortcuts = shared.shortcuts();
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(egui::Color32::BLACK))
        .show(ui, |ui| {
            popout_content_ui(ui, frame);
        });
    let ctx = ui.ctx();
    let mut fired = false;
    for action in shortcuts.poll(ctx) {
        shared.push_action(action);
        fired = true;
    }
    if fired {
        // The editor window owns the playhead, so it has to run a pass before
        // anything the user just pressed becomes visible.
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
    if ctx.input(|input| input.viewport().close_requested()) {
        shared.request_close();
    }
    shared.count_frame();
}

/// The pop-out window's contents: the picture, fitted to the whole window.
///
/// There is no scrub bar and no transport row. The pop-out is the picture on
/// a second display, and the editor window keeps the controls.
pub fn popout_content_ui(ui: &mut egui::Ui, frame: Option<ViewerFrame>) {
    paint_picture(ui, ui.available_size(), frame);
}

/// The Viewer menu's pop-out item. Returns whether it was clicked.
///
/// The one item is both directions, as a docked panel's own menu item would
/// be: it pops the viewer out, and once it is out it puts it back.
pub fn popout_menu_ui(ui: &mut egui::Ui, popout: &mut PopoutViewer) -> bool {
    let label = if popout.is_open() {
        CLOSE_LABEL
    } else {
        OPEN_LABEL
    };
    let clicked = ui.button(label).clicked();
    if clicked {
        popout.toggle();
        ui.close();
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::{
        POPOUT_TITLE, PopoutShared, PopoutViewer, is_playback_action, playback_shortcuts,
        popout_viewport_builder, popout_viewport_ui,
    };
    use crate::shortcuts::{Action, ShortcutMap};
    use crate::viewer::ViewerFrame;
    use eframe::egui;

    /// A picture standing in for the compositor output.
    fn preview() -> ViewerFrame {
        ViewerFrame::new(egui::TextureId::User(11), 1920, 1080)
    }

    /// One headless pop-out pass over `shared`, with `events` as its input.
    fn popout_frame(shared: &PopoutShared, events: Vec<egui::Event>) {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(960.0, 540.0),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| popout_viewport_ui(ui, shared));
        let _ = ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        output.textures_delta.clear();
    }

    /// A key press, as egui reports one.
    fn press(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn a_new_popout_is_closed_and_empty() {
        let popout = PopoutViewer::new();
        assert!(!popout.is_open(), "the viewer starts docked");
        assert_eq!(popout.shared().frame(), None, "with nothing to paint");
        assert_eq!(popout.shared().frames_painted(), 0);
    }

    #[test]
    fn toggling_opens_and_closes_it() {
        let mut popout = PopoutViewer::new();
        assert!(popout.toggle(), "the first toggle pops the viewer out");
        assert!(popout.is_open());
        assert!(!popout.toggle(), "and the second puts it back");
        assert!(!popout.is_open());
    }

    #[test]
    fn the_published_picture_is_the_texture_the_dock_paints() {
        let popout = PopoutViewer::new();
        popout.publish(preview());
        assert_eq!(
            popout.shared().frame(),
            Some(preview()),
            "the pop-out samples the compositor's own texture, not a copy"
        );
    }

    #[test]
    fn closing_the_window_returns_the_viewer_to_the_dock() {
        let mut popout = PopoutViewer::new();
        popout.open();
        popout.publish(preview());
        popout.shared().request_close();
        assert!(!popout.poll_close(), "the close reaches the main pass");
        assert!(!popout.is_open(), "and the viewer is docked again");
        assert_eq!(
            popout.shared().frame(),
            None,
            "the closed pop-out holds no picture"
        );
    }

    #[test]
    fn a_stale_close_does_not_shut_the_next_popout() {
        let mut popout = PopoutViewer::new();
        popout.shared().request_close();
        popout.open();
        assert!(popout.poll_close(), "the pop-out opened and stayed open");
    }

    #[test]
    fn only_playback_actions_are_claimed() {
        assert!(is_playback_action(Action::TogglePlayback));
        assert!(is_playback_action(Action::PlayForward));
        assert!(is_playback_action(Action::StepForward));
        assert!(is_playback_action(Action::GoToEnd));
        assert!(
            !is_playback_action(Action::Undo),
            "editing stays in the editor window"
        );

        let playback = playback_shortcuts(&ShortcutMap::default_map());
        assert!(
            playback
                .bindings()
                .iter()
                .all(|binding| is_playback_action(binding.action)),
            "the pop-out's map holds nothing else"
        );
        assert!(
            playback.chord_for(Action::TogglePlayback).is_some(),
            "and it keeps the transport bindings it needs"
        );
    }

    #[test]
    fn a_non_playback_action_is_not_queued() {
        let shared = PopoutShared::new();
        shared.push_action(Action::Undo);
        assert!(
            shared.take_actions().is_empty(),
            "the pop-out has no project to edit"
        );
    }

    #[test]
    fn the_popout_window_queues_the_playback_keys_it_sees() {
        let shared = PopoutShared::new();
        shared.set_frame(preview());
        shared.set_shortcuts(&playback_shortcuts(&ShortcutMap::default_map()));
        popout_frame(
            &shared,
            vec![press(egui::Key::Space), press(egui::Key::ArrowRight)],
        );
        assert_eq!(
            shared.take_actions(),
            vec![Action::TogglePlayback, Action::StepForward],
            "the pop-out passes playback keys back to the editor"
        );
        assert_eq!(shared.frames_painted(), 1, "and it painted its frame");
        assert!(
            shared.take_actions().is_empty(),
            "taking the queue empties it"
        );
    }

    #[test]
    fn a_placed_popout_asks_for_that_position() {
        let mut popout = PopoutViewer::new();
        assert_eq!(popout.position(), None, "placement is the WM's by default");
        popout.set_position([1280.0, 0.0]);
        assert_eq!(popout.position(), Some([1280.0, 0.0]));

        let builder = popout_viewport_builder(popout.position());
        assert_eq!(
            builder.position,
            Some(egui::pos2(1280.0, 0.0)),
            "the second monitor's origin should reach the window"
        );
        assert_eq!(builder.title.as_deref(), Some(POPOUT_TITLE));
        assert_eq!(
            popout_viewport_builder(None).position,
            None,
            "an unplaced pop-out should not pin itself to the origin"
        );
    }

    #[test]
    fn the_popout_window_paints_without_a_picture() {
        let shared = PopoutShared::new();
        shared.set_shortcuts(&playback_shortcuts(&ShortcutMap::default_map()));
        popout_frame(&shared, Vec::new());
        assert_eq!(
            shared.frames_painted(),
            1,
            "a pop-out opened before the first composite paints black"
        );
    }
}
