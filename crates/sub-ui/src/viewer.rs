//! The viewer panel: the compositor's picture, a scrub bar, frame stepping
//! and a timecode readout.
//!
//! This is the first user-visible half of milestone 1 (docs/PLAN.md §5.7):
//! open a sequence and scrub it. The panel paints the texture
//! [`sub_render::Compositor`] renders into, scaled to fit its area with the
//! canvas aspect preserved, and drives the playhead from the scrub bar, the
//! arrow keys and Home/End.
//!
//! As in [`crate::timeline`], the model is kept free of egui and free of
//! floats: the playhead is a frame number at the sequence timebase, every
//! pixel-to-frame conversion is integer arithmetic, and a float appears only
//! where a `Painter` demands one. The playhead is view state, not project
//! state — it is not a project mutation, so it is not a Command and is not
//! undoable.
//!
//! ```
//! use sub_time::{Rational, RationalTime};
//! use sub_ui::viewer::ViewerState;
//!
//! let mut state = ViewerState::new(Rational::FPS_24);
//! state.set_duration(RationalTime::new(48, Rational::FPS_24));
//!
//! // Half way along a 240-pixel scrub bar is half way through the sequence.
//! state.scrub_to_pixel(120, 240);
//! assert_eq!(state.playhead(), RationalTime::new(24, Rational::FPS_24));
//! assert_eq!(state.timecode_label(), "00:00:01:00");
//!
//! // Stepping stops at the last frame rather than running past the end.
//! state.go_to_end();
//! assert_eq!(state.step_frames(1), false);
//! assert_eq!(state.playhead(), RationalTime::new(47, Rational::FPS_24));
//! ```

use eframe::egui;
use sub_audio::MeterLevels;
use sub_edit::playback::PlaybackScheduler;
use sub_model::sequence::Sequence;
use sub_time::{Rational, RationalTime, Rounding, Timecode, TimecodeRate};

use crate::meter::MeterState;
use crate::shortcuts::{Action, default_action_for};

/// Height of the scrub bar, in points.
const SCRUB_HEIGHT: f32 = 18.0;

/// How wide the master meter in the transport row is, in points.
const MASTER_METER_WIDTH: f32 = 120.0;

/// How tall the master meter in the transport row is, in points.
const MASTER_METER_HEIGHT: f32 = 8.0;

/// What the docked panel says while the picture is in the pop-out window.
pub const POPPED_OUT_LABEL: &str = "Showing in the pop-out window";

/// Paints `frame` into a `size` area: black everywhere, with the canvas
/// fitted into the middle, and returns the area it took.
///
/// This is the whole of drawing the preview, and it is a free function
/// because the docked viewer panel and the pop-out window
/// ([`crate::popout`]) both call it with the same [`egui::TextureId`]. The
/// texture is sampled, never copied, so a second window costs one more quad
/// and no render pass of its own.
pub fn paint_picture(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    frame: Option<ViewerFrame>,
) -> egui::Rect {
    let (area, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 0.0, egui::Color32::BLACK);
    let Some(frame) = frame else {
        return area;
    };
    let fit = ViewerFit::new(area.width(), area.height(), frame.width, frame.height);
    if fit.is_empty() {
        return area;
    }
    let mut mesh = egui::Mesh::with_texture(frame.texture);
    mesh.add_rect_with_uv(
        fit.centred_in(area),
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    painter.add(egui::Shape::mesh(mesh));
    area
}

/// Casts a pixel or frame count to a float, at the painting boundary only.
#[allow(
    clippy::cast_precision_loss,
    reason = "painting coordinates are floats; the frame arithmetic above them is exact"
)]
fn as_f32(value: i64) -> f32 {
    value as f32
}

/// A picture ready for the viewer to sample: the compositor's output
/// registered with egui, and the canvas it was rendered at.
///
/// The texture is sampled, not copied — egui and the compositor share one
/// wgpu device (docs/PLAN.md §3) — so the same handle can be handed back
/// every frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewerFrame {
    /// The compositor output, registered with egui's renderer.
    pub texture: egui::TextureId,
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
}

impl ViewerFrame {
    /// Describes `texture` as a `width` x `height` canvas.
    #[must_use]
    pub const fn new(texture: egui::TextureId, width: u32, height: u32) -> Self {
        Self {
            texture,
            width,
            height,
        }
    }
}

/// The canvas scaled to fit an area with its aspect ratio preserved.
///
/// The fit *contains*: the whole picture is visible and the unused part of
/// the area stays black, pillarboxed for a narrower canvas and letterboxed
/// for a wider one. Nothing is cropped and nothing is stretched, which is
/// what "correct aspect" means for a viewer.
///
/// [`sub_render::LetterboxFit`] does the same job one stage earlier, fitting
/// a source picture into the integer sequence canvas; this fits that canvas
/// into a float area of screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewerFit {
    /// Fitted width in points.
    width: f32,
    /// Fitted height in points.
    height: f32,
}

impl ViewerFit {
    /// Nothing to draw: an empty area or a degenerate canvas.
    pub const EMPTY: Self = Self {
        width: 0.0,
        height: 0.0,
    };

    /// Fits a `canvas_width` x `canvas_height` canvas into an
    /// `area_width` x `area_height` area.
    ///
    /// A zero canvas dimension, or an area that is empty or not finite,
    /// yields [`ViewerFit::EMPTY`], which draws nothing.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        reason = "a canvas dimension is a pixel count being turned into a paint size"
    )]
    pub fn new(area_width: f32, area_height: f32, canvas_width: u32, canvas_height: u32) -> Self {
        if canvas_width == 0 || canvas_height == 0 {
            return Self::EMPTY;
        }
        if !area_width.is_finite() || !area_height.is_finite() {
            return Self::EMPTY;
        }
        if area_width <= 0.0 || area_height <= 0.0 {
            return Self::EMPTY;
        }
        let canvas_width = canvas_width as f32;
        let canvas_height = canvas_height as f32;
        let scale = (area_width / canvas_width).min(area_height / canvas_height);
        Self {
            width: canvas_width * scale,
            height: canvas_height * scale,
        }
    }

    /// The fitted width, in points.
    #[must_use]
    pub const fn width(self) -> f32 {
        self.width
    }

    /// The fitted height, in points.
    #[must_use]
    pub const fn height(self) -> f32 {
        self.height
    }

    /// True when there is nothing to paint.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    /// The fitted rectangle, centred in `area`.
    #[must_use]
    pub fn centred_in(self, area: egui::Rect) -> egui::Rect {
        egui::Rect::from_center_size(area.center(), egui::vec2(self.width, self.height))
    }
}

/// Something the viewer's keyboard map asks of the playhead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerAction {
    /// Step this many frames, backwards when negative.
    StepFrames(i64),
    /// Jump to the first frame.
    GoToStart,
    /// Jump to the last frame.
    GoToEnd,
}

impl ViewerAction {
    /// The playhead move `action` asks for, if it asks for one at all.
    ///
    /// The playback and editing actions are somebody else's; the viewer only
    /// answers the four that move the playhead.
    #[must_use]
    pub const fn for_action(action: Action) -> Option<Self> {
        match action {
            Action::StepBack => Some(Self::StepFrames(-1)),
            Action::StepForward => Some(Self::StepFrames(1)),
            Action::GoToStart => Some(Self::GoToStart),
            Action::GoToEnd => Some(Self::GoToEnd),
            _ => None,
        }
    }
}

/// Something the keyboard map asks of the playback transport.
///
/// Like [`ViewerAction`], it is the pure translation from a bound action to
/// what the transport should do, so the mapping is testable without egui and
/// without a running clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportAction {
    /// L: play forward, shuttling faster on every repeat.
    PlayForward,
    /// J: play backwards, shuttling faster on every repeat.
    PlayBackward,
    /// K: stop where the playhead stands.
    Pause,
    /// Space: play at 1x forward, or pause if anything is playing.
    Toggle,
}

impl TransportAction {
    /// The transport move `action` asks for, if it asks for one at all.
    #[must_use]
    pub const fn for_action(action: Action) -> Option<Self> {
        match action {
            Action::PlayForward => Some(Self::PlayForward),
            Action::PlayBackward => Some(Self::PlayBackward),
            Action::PausePlayback => Some(Self::Pause),
            Action::TogglePlayback => Some(Self::Toggle),
            _ => None,
        }
    }

    /// Runs this action on `scheduler`.
    pub fn apply(self, scheduler: &mut PlaybackScheduler) {
        match self {
            Self::PlayForward => scheduler.play_forward(),
            Self::PlayBackward => scheduler.play_backward(),
            Self::Pause => scheduler.pause(),
            Self::Toggle => scheduler.toggle(),
        }
    }
}

/// The action a key press asks for, if any.
///
/// The binding itself lives in [`crate::shortcuts`], so the viewer, the help
/// window and any future remapping all read one table. Modifiers are matched
/// exactly there, so a shortcut that happens to use an arrow key elsewhere in
/// the app keeps working.
#[must_use]
pub fn action_for_key(key: egui::Key, modifiers: egui::Modifiers) -> Option<ViewerAction> {
    ViewerAction::for_action(default_action_for(key, modifiers)?)
}

/// The keys the viewer claims, in the order they are polled.
const VIEWER_KEYS: [egui::Key; 4] = [
    egui::Key::ArrowLeft,
    egui::Key::ArrowRight,
    egui::Key::Home,
    egui::Key::End,
];

/// Where the playhead is, how long the sequence is, and how to say both in
/// timecode.
///
/// Both are held as frame numbers at the sequence timebase, so scrubbing and
/// stepping are exact integer moves and the playhead can never land between
/// frames.
#[derive(Debug, Clone, Copy)]
pub struct ViewerState {
    /// The sequence timebase every time is expressed at.
    rate: Rational,
    /// How to label a frame, or `None` at a rate timecode cannot label.
    timecode_rate: Option<TimecodeRate>,
    /// Sequence length in frames; never negative.
    duration_frames: i64,
    /// The playhead, in frames from zero; in `0..=last_frame()`.
    frame: i64,
}

impl ViewerState {
    /// A viewer of an empty sequence with timebase `rate`, parked at frame
    /// zero.
    #[must_use]
    pub fn new(rate: Rational) -> Self {
        Self {
            rate,
            timecode_rate: default_timecode_rate(rate),
            duration_frames: 0,
            frame: 0,
        }
    }

    /// A viewer of `sequence`: its timebase, its length and frame zero.
    #[must_use]
    pub fn for_sequence(sequence: &Sequence) -> Self {
        let mut state = Self::new(sequence.settings.frame_rate);
        state.set_duration(sequence_duration(sequence));
        state
    }

    /// The sequence timebase.
    #[must_use]
    pub const fn rate(&self) -> Rational {
        self.rate
    }

    /// How frames are labelled, or `None` at a rate timecode cannot label —
    /// where [`ViewerState::timecode_label`] falls back to a frame count.
    #[must_use]
    pub const fn timecode_rate(&self) -> Option<TimecodeRate> {
        self.timecode_rate
    }

    /// Re-expresses the viewer at a new timebase, rescaling the playhead and
    /// the duration so both keep pointing at the same instant.
    pub fn set_rate(&mut self, rate: Rational) {
        if rate == self.rate {
            return;
        }
        let playhead = self.playhead().rescaled_to(rate);
        let duration = self.duration().rescaled_to_rounding(rate, Rounding::Ceil);
        self.rate = rate;
        self.timecode_rate = default_timecode_rate(rate);
        self.duration_frames = duration.value().max(0);
        self.seek_to(playhead);
    }

    /// The sequence length.
    #[must_use]
    pub const fn duration(&self) -> RationalTime {
        RationalTime::new(self.duration_frames, self.rate)
    }

    /// The sequence length in frames.
    #[must_use]
    pub const fn duration_frames(&self) -> i64 {
        self.duration_frames
    }

    /// Sets the sequence length, clamping the playhead back inside it.
    ///
    /// A duration that is not a whole number of frames at the timebase is
    /// rounded up: a part-frame at the end is still a frame to show. A
    /// negative duration is treated as empty.
    pub fn set_duration(&mut self, duration: RationalTime) {
        let frames = duration
            .rescaled_to_rounding(self.rate, Rounding::Ceil)
            .value();
        self.duration_frames = frames.max(0);
        self.frame = self.frame.clamp(0, self.last_frame_number());
    }

    /// The last frame that can be shown, as a frame number; zero for an empty
    /// sequence.
    #[must_use]
    pub const fn last_frame_number(&self) -> i64 {
        if self.duration_frames <= 1 {
            0
        } else {
            self.duration_frames - 1
        }
    }

    /// The last frame that can be shown.
    #[must_use]
    pub const fn last_frame(&self) -> RationalTime {
        RationalTime::new(self.last_frame_number(), self.rate)
    }

    /// The playhead.
    #[must_use]
    pub const fn playhead(&self) -> RationalTime {
        RationalTime::new(self.frame, self.rate)
    }

    /// The playhead as a frame number at the timebase.
    #[must_use]
    pub const fn playhead_frame(&self) -> i64 {
        self.frame
    }

    /// Moves the playhead to `time`, rescaled to the timebase and clamped
    /// into the sequence.
    ///
    /// Returns true if the playhead moved, which is what tells the app a new
    /// frame has to be composited.
    pub fn seek_to(&mut self, time: RationalTime) -> bool {
        let frame = time.rescaled_to(self.rate).value();
        self.seek_to_frame(frame)
    }

    /// Moves the playhead to frame `frame`, clamped into the sequence.
    ///
    /// Returns true if the playhead moved.
    pub fn seek_to_frame(&mut self, frame: i64) -> bool {
        let frame = frame.clamp(0, self.last_frame_number());
        let moved = frame != self.frame;
        self.frame = frame;
        moved
    }

    /// Steps `delta` frames, backwards when negative, stopping at either end.
    ///
    /// Returns true if the playhead moved.
    pub fn step_frames(&mut self, delta: i64) -> bool {
        self.seek_to_frame(self.frame.saturating_add(delta))
    }

    /// Jumps to the first frame. Returns true if the playhead moved.
    pub fn go_to_start(&mut self) -> bool {
        self.seek_to_frame(0)
    }

    /// Jumps to the last frame. Returns true if the playhead moved.
    pub fn go_to_end(&mut self) -> bool {
        self.seek_to_frame(self.last_frame_number())
    }

    /// Applies a keyboard action. Returns true if the playhead moved.
    pub fn apply(&mut self, action: ViewerAction) -> bool {
        match action {
            ViewerAction::StepFrames(delta) => self.step_frames(delta),
            ViewerAction::GoToStart => self.go_to_start(),
            ViewerAction::GoToEnd => self.go_to_end(),
        }
    }

    /// The frame the pixel column `px` of a `width_px`-wide scrub bar covers.
    ///
    /// Each column covers an equal span of the sequence, as on the timeline,
    /// so dragging one pixel at a time walks the sequence monotonically.
    /// Columns outside the bar clamp to its ends.
    #[must_use]
    pub fn frame_at_pixel(&self, px: i64, width_px: u32) -> i64 {
        if width_px == 0 || self.duration_frames <= 0 {
            return 0;
        }
        if px <= 0 {
            return 0;
        }
        let frames = i128::from(px) * i128::from(self.duration_frames);
        let frame = frames.div_euclid(i128::from(width_px));
        let frame = i64::try_from(frame).unwrap_or(i64::MAX);
        frame.clamp(0, self.last_frame_number())
    }

    /// The instant the pixel column `px` of a `width_px`-wide scrub bar
    /// covers.
    #[must_use]
    pub fn time_at_pixel(&self, px: i64, width_px: u32) -> RationalTime {
        RationalTime::new(self.frame_at_pixel(px, width_px), self.rate)
    }

    /// Moves the playhead to the pixel column `px` of a `width_px`-wide scrub
    /// bar. Returns true if the playhead moved.
    pub fn scrub_to_pixel(&mut self, px: i64, width_px: u32) -> bool {
        self.seek_to_frame(self.frame_at_pixel(px, width_px))
    }

    /// Where to paint the playhead on a `width_px`-wide scrub bar, in pixels
    /// from its left edge.
    ///
    /// The one float in the model, and the last step of the computation: the
    /// position is worked out exactly in integers and divided once.
    #[must_use]
    pub fn playhead_pixel(&self, width_px: u32) -> f32 {
        if width_px == 0 || self.duration_frames <= 0 {
            return 0.0;
        }
        as_f32(self.frame) * as_f32(i64::from(width_px)) / as_f32(self.duration_frames)
    }

    /// How far through the sequence the playhead is, in `0.0..=1.0`.
    ///
    /// For a progress bar only; every decision is made on frame numbers.
    #[must_use]
    pub fn progress(&self) -> f32 {
        if self.duration_frames <= 0 {
            return 0.0;
        }
        (as_f32(self.frame) / as_f32(self.duration_frames)).clamp(0.0, 1.0)
    }

    /// The playhead's timecode, or `None` at a rate timecode cannot label or
    /// past the 24-hour wrap.
    #[must_use]
    pub fn timecode(&self) -> Option<Timecode> {
        self.timecode_at(self.frame)
    }

    /// The timecode of frame `frame`, if it has one.
    #[must_use]
    pub fn timecode_at(&self, frame: i64) -> Option<Timecode> {
        let rate = self.timecode_rate?;
        Timecode::from_rational_time(RationalTime::new(frame, self.rate), rate).ok()
    }

    /// The playhead, as the viewer displays it.
    ///
    /// Timecode where the rate can be labelled, and a plain frame count
    /// otherwise, so the readout is never blank.
    #[must_use]
    pub fn timecode_label(&self) -> String {
        self.label_at(self.frame)
    }

    /// The sequence length, as the viewer displays it.
    #[must_use]
    pub fn duration_label(&self) -> String {
        self.label_at(self.duration_frames)
    }

    /// The label for frame `frame`.
    fn label_at(&self, frame: i64) -> String {
        self.timecode_at(frame)
            .map_or_else(|| format!("frame {frame}"), |code| code.to_string())
    }
}

/// The timecode rate a sequence timebase is labelled at, or `None` when
/// timecode cannot label it.
///
/// NTSC rates take drop-frame counting, which is what an editor expects of
/// 29.97 and 59.94; every other labellable rate is non-drop.
fn default_timecode_rate(rate: Rational) -> Option<TimecodeRate> {
    TimecodeRate::new(rate, TimecodeRate::rate_drops_frames(rate)).ok()
}

/// How long `sequence` is: the end of its longest track.
///
/// An empty sequence is zero long, and the viewer then shows a single black
/// frame rather than a scrub bar over nothing.
#[must_use]
pub fn sequence_duration(sequence: &Sequence) -> RationalTime {
    let rate = sequence.settings.frame_rate;
    sequence
        .tracks
        .iter()
        .map(|track| track.duration(rate))
        .max_by_key(|duration| duration.value())
        .unwrap_or_else(|| RationalTime::zero(rate))
}

/// The viewer panel: the picture, the transport row and the scrub bar.
///
/// The panel owns nothing but view state. It never blocks on media: it paints
/// whichever [`ViewerFrame`] the caller has ready, and reports the playhead
/// the caller should composite next.
#[derive(Debug, Clone, Copy)]
pub struct ViewerPanel {
    /// The playhead and the sequence it moves over.
    pub state: ViewerState,
    /// Whether the panel claims the arrow and Home/End keys.
    pub keyboard: bool,
    /// Whether the picture is showing in the pop-out window instead.
    pub popped_out: bool,
    /// The master bus meter, fed from the mixer's meter bank.
    pub master_meter: MeterState,
}

impl ViewerPanel {
    /// A panel over a sequence with timebase `rate`, with the keyboard live.
    #[must_use]
    pub fn new(rate: Rational) -> Self {
        Self {
            state: ViewerState::new(rate),
            keyboard: true,
            popped_out: false,
            master_meter: MeterState::new(),
        }
    }

    /// A panel over `sequence`.
    #[must_use]
    pub fn for_sequence(sequence: &Sequence) -> Self {
        Self {
            state: ViewerState::for_sequence(sequence),
            keyboard: true,
            popped_out: false,
            master_meter: MeterState::new(),
        }
    }

    /// Feeds the master meter with the levels the audio callback published,
    /// `elapsed` seconds after the last frame.
    pub fn update_master_meter(&mut self, levels: MeterLevels, elapsed: f32) {
        self.master_meter.update(levels, elapsed);
    }

    /// Draws the panel and reports whether the playhead moved.
    ///
    /// A `true` return means the caller must composite [`ViewerState`]'s new
    /// playhead before the next paint.
    pub fn ui(&mut self, ui: &mut egui::Ui, frame: Option<ViewerFrame>) -> bool {
        let mut moved = self.read_keyboard(ui);
        moved |= self.transport_ui(ui);
        // The scrub bar sits under the picture, so it is laid out from the
        // bottom up: reserve its height, then give the rest to the canvas.
        let available = ui.available_size();
        let picture_height = (available.y - SCRUB_HEIGHT - ui.spacing().item_spacing.y).max(0.0);
        self.picture_ui(ui, egui::vec2(available.x, picture_height), frame);
        moved |= self.scrub_ui(ui);
        moved
    }

    /// Handles the keys the panel claims, once per press, repeats included.
    fn read_keyboard(&mut self, ui: &egui::Ui) -> bool {
        if !self.keyboard || ui.ctx().egui_wants_keyboard_input() {
            return false;
        }
        let mut moved = false;
        for key in VIEWER_KEYS {
            let Some(action) = action_for_key(key, egui::Modifiers::NONE) else {
                continue;
            };
            while ui
                .ctx()
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, key))
            {
                moved |= self.state.apply(action);
            }
        }
        moved
    }

    /// The timecode readout and the step buttons.
    fn transport_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut moved = false;
        ui.horizontal(|ui| {
            ui.monospace(self.state.timecode_label());
            ui.label("/");
            ui.monospace(self.state.duration_label());
            if ui
                .button("|<")
                .on_hover_text("First frame (Home)")
                .clicked()
            {
                moved |= self.state.go_to_start();
            }
            if ui
                .button("<")
                .on_hover_text("Previous frame (Left arrow)")
                .clicked()
            {
                moved |= self.state.step_frames(-1);
            }
            if ui
                .button(">")
                .on_hover_text("Next frame (Right arrow)")
                .clicked()
            {
                moved |= self.state.step_frames(1);
            }
            if ui.button(">|").on_hover_text("Last frame (End)").clicked() {
                moved |= self.state.go_to_end();
            }
            // The master meter takes the right-hand end of the transport row,
            // where it is next to the picture it belongs to.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let width = ui.available_width().min(MASTER_METER_WIDTH);
                if width > 0.0 {
                    self.master_meter.ui(ui, width, MASTER_METER_HEIGHT);
                }
            });
        });
        moved
    }

    /// The picture, or a note that it is showing in the pop-out window.
    fn picture_ui(&self, ui: &mut egui::Ui, size: egui::Vec2, frame: Option<ViewerFrame>) {
        if !self.popped_out {
            paint_picture(ui, size, frame);
            return;
        }
        // The picture is on the other window; the panel keeps its place in the
        // dock and says where it went, so the tab is never a blank rectangle.
        let area = paint_picture(ui, size, None);
        ui.painter_at(area).text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            POPPED_OUT_LABEL,
            egui::FontId::proportional(14.0),
            ui.visuals().weak_text_color(),
        );
    }

    /// The scrub bar: a track, the elapsed part of it, and the playhead.
    fn scrub_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let (bar, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), SCRUB_HEIGHT),
            egui::Sense::click_and_drag(),
        );
        let width_px = pixels_of(bar.width());
        let mut moved = false;
        if let Some(pointer) = response.interact_pointer_pos()
            && (response.dragged() || response.clicked())
        {
            moved |= self
                .state
                .scrub_to_pixel(pixel_offset(pointer.x - bar.left()), width_px);
        }

        let visuals = ui.visuals();
        let painter = ui.painter_at(bar);
        painter.rect_filled(bar, 2.0, visuals.extreme_bg_color);
        let head = bar.left() + self.state.playhead_pixel(width_px);
        let elapsed = egui::Rect::from_min_max(bar.min, egui::pos2(head, bar.max.y));
        painter.rect_filled(elapsed, 2.0, visuals.selection.bg_fill);
        painter.line_segment(
            [egui::pos2(head, bar.top()), egui::pos2(head, bar.bottom())],
            egui::Stroke::new(2.0, visuals.strong_text_color()),
        );
        moved
    }
}

/// A widget width in points as a whole number of pixel columns.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a widget width is small, non-negative and only ever a column count"
)]
fn pixels_of(width: f32) -> u32 {
    if width.is_finite() && width > 0.0 {
        width as u32
    } else {
        0
    }
}

/// A pointer offset within a widget as a pixel column.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a pointer offset is bounded by the window size"
)]
fn pixel_offset(offset: f32) -> i64 {
    if offset.is_finite() {
        offset.floor() as i64
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TransportAction, ViewerAction, ViewerFit, ViewerFrame, ViewerPanel, ViewerState,
        action_for_key,
    };
    use crate::shortcuts::Action;
    use eframe::egui;
    use sub_audio::MeterLevels;
    use sub_edit::playback::{PlaybackScheduler, ShuttleSpeed};
    use sub_model::sequence::{Sequence, SequenceSettings};
    use sub_time::{Rational, RationalTime};

    /// The area the headless context paints into.
    const SCREEN: egui::Vec2 = egui::vec2(800.0, 600.0);

    /// A 1080p canvas standing in for the compositor output.
    fn preview() -> ViewerFrame {
        ViewerFrame::new(egui::TextureId::User(7), 1920, 1080)
    }

    /// Raw input over a fixed screen, so the panel's layout is predictable.
    fn raw_input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
            events,
            ..egui::RawInput::default()
        }
    }

    /// One unmodified key press.
    fn press(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Every string a shape tree draws.
    fn collect_text(shape: &egui::Shape, into: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => into.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, into);
                }
            }
            _ => {}
        }
    }

    /// Every textured quad a shape tree draws, as (texture, bounds).
    fn collect_meshes(shape: &egui::Shape, into: &mut Vec<(egui::TextureId, egui::Rect)>) {
        match shape {
            egui::Shape::Mesh(mesh) => into.push((mesh.texture_id, mesh.calc_bounds())),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_meshes(shape, into);
                }
            }
            _ => {}
        }
    }

    /// Every filled rectangle in `shape`, with the colour it was filled in.
    fn collect_rects(shape: &egui::Shape, into: &mut Vec<(egui::Rect, egui::Color32)>) {
        match shape {
            egui::Shape::Rect(rect) => into.push((rect.rect, rect.fill)),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_rects(shape, into);
                }
            }
            _ => {}
        }
    }

    /// What one painted frame drew, and whether the playhead moved.
    struct Painted {
        texts: Vec<String>,
        meshes: Vec<(egui::TextureId, egui::Rect)>,
        rects: Vec<(egui::Rect, egui::Color32)>,
        moved: bool,
    }

    /// Paints the panel into a headless egui context. Two frames, because the
    /// first lays the fonts out; the events go to the second.
    fn paint(panel: &mut ViewerPanel, events: &[egui::Event]) -> Painted {
        let ctx = egui::Context::default();
        let mut painted = Painted {
            texts: Vec::new(),
            meshes: Vec::new(),
            rects: Vec::new(),
            moved: false,
        };
        for frame in 0..2 {
            let input = if frame == 0 {
                raw_input(Vec::new())
            } else {
                raw_input(events.to_vec())
            };
            painted.texts.clear();
            painted.meshes.clear();
            painted.rects.clear();
            let mut moved = false;
            let mut output = ctx.run_ui(input, |ui| {
                moved = panel.ui(ui, Some(preview()));
            });
            painted.moved = moved;
            for clipped in &output.shapes {
                collect_text(&clipped.shape, &mut painted.texts);
                collect_meshes(&clipped.shape, &mut painted.meshes);
                collect_rects(&clipped.shape, &mut painted.rects);
            }
            // No painter here consumes the font atlas, so release it by hand
            // rather than let epaint panic on the unapplied delta.
            output.textures_delta.clear();
        }
        painted
    }

    fn state_of(frames: i64, rate: Rational) -> ViewerState {
        let mut state = ViewerState::new(rate);
        state.set_duration(RationalTime::new(frames, rate));
        state
    }

    #[test]
    fn a_wider_canvas_is_letterboxed_and_a_narrower_one_pillarboxed() {
        // 16:9 into a square area: full width, bars above and below.
        let wide = ViewerFit::new(400.0, 400.0, 1920, 1080);
        assert!((wide.width() - 400.0).abs() < 0.01);
        assert!((wide.height() - 225.0).abs() < 0.01);

        // 9:16 into the same square: full height, bars either side.
        let tall = ViewerFit::new(400.0, 400.0, 1080, 1920);
        assert!((tall.height() - 400.0).abs() < 0.01);
        assert!((tall.width() - 225.0).abs() < 0.01);
    }

    #[test]
    fn a_matching_aspect_fills_the_area_exactly() {
        let fit = ViewerFit::new(640.0, 360.0, 3840, 2160);
        assert!((fit.width() - 640.0).abs() < 0.01);
        assert!((fit.height() - 360.0).abs() < 0.01);
    }

    #[test]
    fn the_fitted_picture_keeps_the_canvas_aspect() {
        let fit = ViewerFit::new(1000.0, 300.0, 1920, 1080);
        let canvas_aspect = 1920.0_f32 / 1080.0;
        assert!((fit.width() / fit.height() - canvas_aspect).abs() < 0.001);
        assert!(fit.width() <= 1000.0 && fit.height() <= 300.0);
    }

    #[test]
    fn a_degenerate_canvas_or_area_draws_nothing() {
        for fit in [
            ViewerFit::new(100.0, 100.0, 0, 1080),
            ViewerFit::new(100.0, 100.0, 1920, 0),
            ViewerFit::new(0.0, 100.0, 1920, 1080),
            ViewerFit::new(100.0, -1.0, 1920, 1080),
            ViewerFit::new(f32::NAN, 100.0, 1920, 1080),
            ViewerFit::new(f32::INFINITY, 100.0, 1920, 1080),
        ] {
            assert!(fit.is_empty(), "{fit:?} should draw nothing");
        }
    }

    #[test]
    fn the_fitted_rectangle_is_centred_in_its_area() {
        let area = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 400.0));
        let rect = ViewerFit::new(400.0, 400.0, 1920, 1080).centred_in(area);
        assert!((rect.center() - area.center()).length() < 0.01);
        assert!((rect.width() - 400.0).abs() < 0.01);
        assert!((rect.top() - (20.0 + 87.5)).abs() < 0.01);
    }

    #[test]
    fn a_fresh_viewer_is_empty_and_parked_at_zero() {
        let state = ViewerState::new(Rational::FPS_24);
        assert_eq!(state.duration_frames(), 0);
        assert_eq!(state.playhead(), RationalTime::new(0, Rational::FPS_24));
        assert_eq!(state.last_frame_number(), 0);
        let mut copy = state;
        assert!(!copy.step_frames(1));
        assert!(state.progress().abs() < f32::EPSILON);
        assert!(state.playhead_pixel(100).abs() < f32::EPSILON);
    }

    #[test]
    fn a_part_frame_duration_rounds_up_to_a_whole_frame() {
        let mut state = ViewerState::new(Rational::FPS_24);
        // Half a second of 48 fps material is 12 frames at 24 fps exactly;
        // one frame of 48 fps material is half a frame here, so it counts.
        state.set_duration(RationalTime::new(25, Rational::new(48, 1).unwrap()));
        assert_eq!(state.duration_frames(), 13);
    }

    #[test]
    fn a_negative_duration_is_treated_as_empty() {
        let mut state = ViewerState::new(Rational::FPS_24);
        state.set_duration(RationalTime::new(-10, Rational::FPS_24));
        assert_eq!(state.duration_frames(), 0);
    }

    #[test]
    fn stepping_stops_at_both_ends() {
        let mut state = state_of(48, Rational::FPS_24);
        assert!(!state.step_frames(-1));
        assert_eq!(state.playhead_frame(), 0);
        assert!(state.step_frames(1));
        assert_eq!(state.playhead_frame(), 1);
        assert!(state.step_frames(i64::MAX));
        assert_eq!(state.playhead_frame(), 47);
        assert!(!state.step_frames(5));
        assert_eq!(state.playhead_frame(), 47);
    }

    #[test]
    fn home_and_end_reach_the_first_and_last_frames() {
        let mut state = state_of(48, Rational::FPS_24);
        assert!(state.go_to_end());
        assert_eq!(state.playhead(), RationalTime::new(47, Rational::FPS_24));
        assert!(!state.go_to_end());
        assert!(state.go_to_start());
        assert_eq!(state.playhead(), RationalTime::new(0, Rational::FPS_24));
    }

    #[test]
    fn seeking_rescales_to_the_timebase_and_clamps() {
        let mut state = state_of(48, Rational::FPS_24);
        // One second of 48 fps material is frame 24 here.
        assert!(state.seek_to(RationalTime::new(48, Rational::new(48, 1).unwrap())));
        assert_eq!(state.playhead_frame(), 24);
        assert!(state.seek_to(RationalTime::new(-5, Rational::FPS_24)));
        assert_eq!(state.playhead_frame(), 0);
        assert!(state.seek_to(RationalTime::new(9_000, Rational::FPS_24)));
        assert_eq!(state.playhead_frame(), 47);
    }

    #[test]
    fn shortening_a_sequence_pulls_the_playhead_back_inside_it() {
        let mut state = state_of(48, Rational::FPS_24);
        state.go_to_end();
        state.set_duration(RationalTime::new(10, Rational::FPS_24));
        assert_eq!(state.playhead_frame(), 9);
    }

    #[test]
    fn the_scrub_bar_maps_pixels_onto_the_sequence_monotonically() {
        let state = state_of(240, Rational::FPS_24);
        assert_eq!(state.frame_at_pixel(0, 480), 0);
        assert_eq!(state.frame_at_pixel(240, 480), 120);
        assert_eq!(state.frame_at_pixel(479, 480), 239);
        // Past either end clamps rather than leaving the sequence.
        assert_eq!(state.frame_at_pixel(-20, 480), 0);
        assert_eq!(state.frame_at_pixel(10_000, 480), 239);
        let mut previous = 0;
        for px in 0..480 {
            let frame = state.frame_at_pixel(px, 480);
            assert!(frame >= previous, "frame went backwards at {px}");
            previous = frame;
        }
    }

    #[test]
    fn the_scrub_bar_and_the_playhead_marker_agree() {
        let mut state = state_of(240, Rational::FPS_24);
        state.scrub_to_pixel(120, 480);
        assert_eq!(state.playhead_frame(), 60);
        assert!((state.playhead_pixel(480) - 120.0).abs() < 0.01);
        assert!((state.progress() - 0.25).abs() < 0.001);
    }

    #[test]
    fn scrubbing_an_empty_or_zero_width_bar_stays_at_zero() {
        let mut empty = ViewerState::new(Rational::FPS_24);
        assert!(!empty.scrub_to_pixel(100, 480));
        let mut state = state_of(240, Rational::FPS_24);
        assert!(!state.scrub_to_pixel(100, 0));
        assert_eq!(state.playhead_frame(), 0);
    }

    #[test]
    fn timecode_labels_come_from_sub_time() {
        let mut state = state_of(48, Rational::FPS_24);
        assert_eq!(state.timecode_label(), "00:00:00:00");
        state.seek_to_frame(25);
        assert_eq!(state.timecode_label(), "00:00:01:01");
        assert_eq!(state.duration_label(), "00:00:02:00");
        assert_eq!(state.timecode().unwrap().frames(), 1);
    }

    #[test]
    fn ntsc_rates_are_labelled_drop_frame() {
        let state = state_of(2, Rational::FPS_29_97);
        assert!(state.timecode_rate().unwrap().is_drop_frame());
        let mut state = state_of(20_000, Rational::FPS_29_97);
        state.seek_to_frame(17_982);
        assert_eq!(state.timecode_label(), "00;10;00;00");
    }

    #[test]
    fn a_rate_timecode_cannot_label_falls_back_to_a_frame_count() {
        let odd = Rational::new(7, 2).unwrap();
        let mut state = state_of(100, odd);
        assert!(state.timecode_rate().is_none());
        state.seek_to_frame(42);
        assert_eq!(state.timecode_label(), "frame 42");
        assert_eq!(state.duration_label(), "frame 100");
    }

    #[test]
    fn changing_the_timebase_keeps_the_playhead_on_the_same_instant() {
        let mut state = state_of(48, Rational::FPS_24);
        state.seek_to_frame(24);
        state.set_rate(Rational::new(48, 1).unwrap());
        assert_eq!(state.rate(), Rational::new(48, 1).unwrap());
        assert_eq!(state.playhead_frame(), 48);
        assert_eq!(state.duration_frames(), 96);
    }

    #[test]
    fn the_keyboard_map_claims_only_unmodified_transport_keys() {
        assert_eq!(
            action_for_key(egui::Key::ArrowLeft, egui::Modifiers::NONE),
            Some(ViewerAction::StepFrames(-1))
        );
        assert_eq!(
            action_for_key(egui::Key::ArrowRight, egui::Modifiers::NONE),
            Some(ViewerAction::StepFrames(1))
        );
        assert_eq!(
            action_for_key(egui::Key::Home, egui::Modifiers::NONE),
            Some(ViewerAction::GoToStart)
        );
        assert_eq!(
            action_for_key(egui::Key::End, egui::Modifiers::NONE),
            Some(ViewerAction::GoToEnd)
        );
        assert_eq!(
            action_for_key(egui::Key::Space, egui::Modifiers::NONE),
            None
        );
        assert_eq!(
            action_for_key(egui::Key::ArrowLeft, egui::Modifiers::COMMAND),
            None
        );
    }

    #[test]
    fn keyboard_actions_move_the_playhead_the_same_way_the_buttons_do() {
        let mut state = state_of(48, Rational::FPS_24);
        assert!(state.apply(ViewerAction::GoToEnd));
        assert_eq!(state.playhead_frame(), 47);
        assert!(state.apply(ViewerAction::StepFrames(-1)));
        assert_eq!(state.playhead_frame(), 46);
        assert!(state.apply(ViewerAction::GoToStart));
        assert_eq!(state.playhead_frame(), 0);
    }

    #[test]
    fn a_panel_takes_its_timebase_and_length_from_the_sequence() {
        let settings = SequenceSettings::default();
        let sequence = Sequence::new("edit", settings);
        let panel = ViewerPanel::for_sequence(&sequence);
        assert_eq!(panel.state.rate(), settings.frame_rate);
        // An empty sequence has nothing to show but frame zero.
        assert_eq!(panel.state.duration_frames(), 0);
        assert_eq!(super::sequence_duration(&sequence).value(), 0);
        assert!(panel.keyboard);
    }

    #[test]
    fn widget_geometry_survives_nonsense_sizes() {
        assert_eq!(super::pixels_of(f32::NAN), 0);
        assert_eq!(super::pixels_of(-3.0), 0);
        assert_eq!(super::pixels_of(640.7), 640);
        assert_eq!(super::pixel_offset(f32::NAN), 0);
        assert_eq!(super::pixel_offset(-0.5), -1);
        assert_eq!(super::pixel_offset(12.9), 12);
    }

    #[test]
    fn the_painted_viewer_draws_the_compositor_texture_at_the_canvas_aspect() {
        let mut panel = ViewerPanel::new(Rational::FPS_24);
        panel
            .state
            .set_duration(RationalTime::new(240, Rational::FPS_24));
        let painted = paint(&mut panel, &[]);
        let quad = painted
            .meshes
            .iter()
            .find(|(texture, _)| *texture == preview().texture)
            .expect("the compositor texture is painted");
        let bounds = quad.1;
        let aspect = bounds.width() / bounds.height();
        assert!(
            (aspect - 1920.0 / 1080.0).abs() < 0.01,
            "the picture is stretched: {bounds:?}"
        );
        assert!(
            bounds.width() <= SCREEN.x + 0.5 && bounds.height() <= SCREEN.y + 0.5,
            "the picture overflows the panel: {bounds:?}"
        );
    }

    #[test]
    fn the_painted_viewer_shows_the_playhead_timecode() {
        let mut panel = ViewerPanel::new(Rational::FPS_24);
        panel
            .state
            .set_duration(RationalTime::new(240, Rational::FPS_24));
        panel.state.seek_to_frame(24);
        let painted = paint(&mut panel, &[]);
        assert!(
            painted.texts.iter().any(|text| text == "00:00:01:00"),
            "the timecode is painted: {:?}",
            painted.texts
        );
        assert!(
            painted.texts.iter().any(|text| text == "00:00:10:00"),
            "the duration is painted: {:?}",
            painted.texts
        );
    }

    #[test]
    fn the_painted_viewer_draws_a_master_meter_that_lights_when_the_master_clips() {
        let mut panel = ViewerPanel::new(Rational::FPS_24);
        panel
            .state
            .set_duration(RationalTime::new(240, Rational::FPS_24));

        let quiet = paint(&mut panel, &[]);
        assert!(
            !quiet
                .rects
                .iter()
                .any(|(_, color)| *color == crate::meter::CLIP_COLOR),
            "a silent master is not lit"
        );

        panel.update_master_meter(MeterLevels::new(1.2, 0.9), 1.0 / 60.0);
        assert!(panel.master_meter.clipping());
        let loud = paint(&mut panel, &[]);
        let lit: Vec<_> = loud
            .rects
            .iter()
            .filter(|(_, color)| *color == crate::meter::CLIP_COLOR)
            .collect();
        assert!(
            !lit.is_empty(),
            "the clipped master meter is painted in the clip colour"
        );
        for (rect, _) in lit {
            assert!(
                rect.width() > 0.0 && rect.height() > 0.0,
                "the meter is drawn with real area: {rect:?}"
            );
        }
    }

    #[test]
    fn the_master_meter_holds_its_peak_and_falls_when_the_transport_stops() {
        let mut panel = ViewerPanel::new(Rational::FPS_24);
        panel.update_master_meter(MeterLevels::new(1.0, 0.7), 1.0 / 60.0);
        let held = panel.master_meter.peak_hold_db();
        panel.update_master_meter(MeterLevels::SILENT, 10.0);
        assert!(
            panel.master_meter.peak_hold_db() < held,
            "the hold falls once the transport has been quiet"
        );
        assert!(!panel.master_meter.clipping());
    }

    #[test]
    fn arrow_keys_and_home_end_drive_the_playhead_through_the_panel() {
        let mut panel = ViewerPanel::new(Rational::FPS_24);
        panel
            .state
            .set_duration(RationalTime::new(240, Rational::FPS_24));

        let painted = paint(&mut panel, &[press(egui::Key::ArrowRight)]);
        assert!(painted.moved);
        assert_eq!(panel.state.playhead_frame(), 1);

        assert!(paint(&mut panel, &[press(egui::Key::End)]).moved);
        assert_eq!(panel.state.playhead_frame(), 239);

        assert!(paint(&mut panel, &[press(egui::Key::ArrowLeft)]).moved);
        assert_eq!(panel.state.playhead_frame(), 238);

        assert!(paint(&mut panel, &[press(egui::Key::Home)]).moved);
        assert_eq!(panel.state.playhead_frame(), 0);

        // A key the viewer does not claim leaves the playhead alone.
        assert!(!paint(&mut panel, &[press(egui::Key::Space)]).moved);
        assert_eq!(panel.state.playhead_frame(), 0);
    }

    #[test]
    fn a_panel_with_the_keyboard_off_ignores_the_transport_keys() {
        let mut panel = ViewerPanel::new(Rational::FPS_24);
        panel
            .state
            .set_duration(RationalTime::new(240, Rational::FPS_24));
        panel.keyboard = false;
        assert!(!paint(&mut panel, &[press(egui::Key::End)]).moved);
        assert_eq!(panel.state.playhead_frame(), 0);
    }

    #[test]
    fn dragging_the_scrub_bar_seeks_the_playhead() {
        let mut panel = ViewerPanel::new(Rational::FPS_24);
        panel
            .state
            .set_duration(RationalTime::new(240, Rational::FPS_24));
        // The scrub bar is the last widget in the panel, so it sits along the
        // bottom edge of the painted area.
        let on_bar = |x: f32| egui::pos2(x, SCREEN.y - super::SCRUB_HEIGHT / 2.0);

        let ctx = egui::Context::default();
        let mut moved = false;
        // Frame 1 lays the fonts out and registers the widget; frame 2 presses
        // on the bar; frame 3 drags along it.
        let frames = [
            Vec::new(),
            vec![
                egui::Event::PointerMoved(on_bar(200.0)),
                egui::Event::PointerButton {
                    pos: on_bar(200.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            vec![egui::Event::PointerMoved(on_bar(400.0))],
        ];
        for events in frames {
            let mut output = ctx.run_ui(raw_input(events), |ui| {
                moved |= panel.ui(ui, Some(preview()));
            });
            output.textures_delta.clear();
        }

        assert!(moved, "the drag moved the playhead");
        // 400 pixels along an 800-pixel bar over 240 frames is frame 120.
        assert_eq!(panel.state.playhead_frame(), 120);
    }

    #[test]
    fn the_transport_keys_drive_the_playback_clock() {
        assert_eq!(
            TransportAction::for_action(Action::PlayForward),
            Some(TransportAction::PlayForward)
        );
        assert_eq!(
            TransportAction::for_action(Action::PlayBackward),
            Some(TransportAction::PlayBackward)
        );
        assert_eq!(
            TransportAction::for_action(Action::PausePlayback),
            Some(TransportAction::Pause)
        );
        assert_eq!(
            TransportAction::for_action(Action::TogglePlayback),
            Some(TransportAction::Toggle)
        );
        // The playhead moves are the viewer's, not the transport's.
        assert_eq!(TransportAction::for_action(Action::StepForward), None);

        let mut scheduler = PlaybackScheduler::new(Rational::FPS_24);
        TransportAction::PlayForward.apply(&mut scheduler);
        assert_eq!(scheduler.speed(), ShuttleSpeed::Forward1x);
        TransportAction::PlayForward.apply(&mut scheduler);
        assert_eq!(scheduler.speed(), ShuttleSpeed::Forward2x);
        TransportAction::PlayBackward.apply(&mut scheduler);
        assert_eq!(scheduler.speed(), ShuttleSpeed::Reverse1x);
        TransportAction::Pause.apply(&mut scheduler);
        assert_eq!(scheduler.speed(), ShuttleSpeed::Paused);
        TransportAction::Toggle.apply(&mut scheduler);
        assert_eq!(scheduler.speed(), ShuttleSpeed::Forward1x);
    }
}
