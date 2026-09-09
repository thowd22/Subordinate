//! The timeline panel: the editing surface, painted straight onto egui's
//! [`Painter`](eframe::egui::Painter).
//!
//! Widgets are deliberately absent from the editing surface. A sequence can
//! hold hundreds of clips and every one of them would otherwise cost an id, a
//! layout pass and an interaction test per frame; painting shapes instead
//! keeps the cost proportional to what is on screen (docs/PLAN.md §5.7).
//!
//! The panel owns no model state. It reads a [`Sequence`] and the [`Project`]
//! the media lives in, and keeps only what looking at them needs: the
//! [`TimelineView`] from [`crate::timeline`], one [`TrackLayout`] index per
//! track, and how far the lanes are scrolled vertically. Every edit still goes
//! through the Command API; nothing here mutates the project.
//!
//! Time never becomes a float. Tick spacing, tick positions and the clip
//! spans are integer frame counts, and floats appear only where a
//! `Painter` demands screen coordinates.

use eframe::egui::{
    Align2, Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2,
    Visuals, pos2,
};
use sub_model::{Clip, MediaItem, Project, Sequence, Track, TrackKind};
use sub_time::{Rational, RationalTime, Timecode, TimecodeRate};

use crate::timeline::{TimelineView, TrackLayout, ZoomLevel};
use crate::track_header::{TrackAction, TrackHeaderState, empty_column_menu};

/// The narrowest a labelled ruler tick may be spaced, in points.
///
/// A timecode label is about eight characters wide, so labels closer than
/// this would collide.
const MIN_LABEL_SPACING_PX: u32 = 96;

/// The narrowest an unlabelled ruler tick may be spaced, in points.
const MIN_TICK_SPACING_PX: u32 = 12;

/// The narrowest clip rectangle that is worth laying text out for.
const MIN_NAME_WIDTH_PX: f32 = 26.0;

/// How wide the bar marking a trimmed clip edge is, in points.
const TRIM_BAR_WIDTH: f32 = 3.0;

/// How much of its colour a clip on a locked track keeps.
///
/// A locked track refuses clip edits (`edit.track_locked`), and the lane has
/// to say so before the edit is attempted rather than after.
const LOCKED_DIM: f32 = 0.45;

/// How far from 1.0 a zoom factor has to be before it counts as a gesture.
const ZOOM_EPSILON: f32 = 0.001;

/// The denominator a wheel zoom factor is approximated over.
const ZOOM_RATIO_DENOMINATOR: u32 = 4096;

/// The fixed sizes the panel lays itself out with, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimelineMetrics {
    /// Height of the timecode ruler across the top.
    pub ruler_height: f32,
    /// Width of the track header column down the left.
    pub header_width: f32,
    /// Height of one track lane.
    pub track_height: f32,
    /// Gap painted between two lanes.
    pub track_gap: f32,
}

impl Default for TimelineMetrics {
    fn default() -> Self {
        Self {
            ruler_height: 26.0,
            header_width: 132.0,
            track_height: 54.0,
            track_gap: 2.0,
        }
    }
}

impl TimelineMetrics {
    /// The vertical distance from one lane's top to the next lane's top.
    #[must_use]
    pub fn lane_pitch(&self) -> f32 {
        self.track_height + self.track_gap
    }

    /// How tall `tracks` lanes are together, gaps included.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        reason = "a track count large enough to lose precision cannot be painted anyway"
    )]
    pub fn lanes_height(&self, tracks: usize) -> f32 {
        if tracks == 0 {
            0.0
        } else {
            tracks as f32 * self.lane_pitch() - self.track_gap
        }
    }
}

/// Where the panel's parts sit inside the rectangle it was given.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelLayout {
    /// The whole panel.
    pub rect: Rect,
    /// The blank square above the track headers.
    pub corner: Rect,
    /// The timecode ruler, right of the corner.
    pub ruler: Rect,
    /// The track header column, below the corner.
    pub headers: Rect,
    /// The lanes clips are painted in: right of the headers, below the ruler.
    pub content: Rect,
}

impl PanelLayout {
    /// Splits `rect` into the ruler, header and content areas.
    #[must_use]
    pub fn new(rect: Rect, metrics: &TimelineMetrics) -> Self {
        let split_x = (rect.left() + metrics.header_width).min(rect.right());
        let split_y = (rect.top() + metrics.ruler_height).min(rect.bottom());
        Self {
            rect,
            corner: Rect::from_min_max(rect.min, pos2(split_x, split_y)),
            ruler: Rect::from_min_max(pos2(split_x, rect.top()), pos2(rect.right(), split_y)),
            headers: Rect::from_min_max(pos2(rect.left(), split_y), pos2(split_x, rect.bottom())),
            content: Rect::from_min_max(pos2(split_x, split_y), rect.max),
        }
    }
}

/// One wheel or pinch gesture, in the terms the panel acts on.
///
/// Pulled out of egui's input state so the scroll and zoom rules can be
/// tested without a window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelInput {
    /// The multiplicative zoom this frame: 1.0 for none, above 1.0 to zoom in.
    pub zoom_factor: f32,
    /// How far the content should move horizontally, in points.
    pub scroll_x: f32,
    /// How far the content should move vertically, in points.
    pub scroll_y: f32,
    /// Where the pointer is, in points from the left edge of the lanes.
    pub anchor_px: f32,
}

impl WheelInput {
    /// A gesture that neither scrolls nor zooms, anchored at `anchor_px`.
    #[must_use]
    pub const fn none(anchor_px: f32) -> Self {
        Self {
            zoom_factor: 1.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            anchor_px,
        }
    }

    /// True if this gesture asks for a zoom rather than a scroll.
    #[must_use]
    pub fn is_zoom(self) -> bool {
        (self.zoom_factor - 1.0).abs() > ZOOM_EPSILON
    }
}

/// What kind of media a clip plays, which is what colours it.
///
/// Colour carries the same meaning everywhere in the editor: picture is blue,
/// sound is green, a still is violet, and anything the project cannot resolve
/// is red so that a missing file is obvious at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClipMediaKind {
    /// Moving picture, with or without sound.
    Video,
    /// Sound only.
    Audio,
    /// A still image: picture with no duration of its own.
    Still,
    /// The media file is missing from disk.
    Offline,
    /// The media has not been probed yet, or is not in this project.
    Unknown,
}

impl ClipMediaKind {
    /// Classifies the media a clip refers to.
    ///
    /// `media` is `None` when the project holds no item with the clip's id,
    /// which a well-formed project never does but a partially loaded one can.
    #[must_use]
    pub fn of(media: Option<&MediaItem>) -> Self {
        let Some(media) = media else {
            return Self::Unknown;
        };
        if media.offline {
            return Self::Offline;
        }
        let Some(info) = media.info.as_ref() else {
            return Self::Unknown;
        };
        match (info.has_video(), info.has_audio()) {
            (true, false) if info.duration.is_none() => Self::Still,
            (true, _) => Self::Video,
            (false, true) => Self::Audio,
            (false, false) => Self::Unknown,
        }
    }

    /// A short word for this kind, used in the track header and in tests.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Still => "still",
            Self::Offline => "offline",
            Self::Unknown => "unknown",
        }
    }

    /// The body colour of a clip of this kind.
    #[must_use]
    pub const fn fill(self) -> Color32 {
        match self {
            Self::Video => Color32::from_rgb(58, 96, 158),
            Self::Audio => Color32::from_rgb(52, 122, 84),
            Self::Still => Color32::from_rgb(104, 74, 146),
            Self::Offline => Color32::from_rgb(150, 54, 54),
            Self::Unknown => Color32::from_rgb(88, 88, 96),
        }
    }

    /// The outline colour of a clip of this kind.
    #[must_use]
    pub const fn outline(self) -> Color32 {
        match self {
            Self::Video => Color32::from_rgb(126, 168, 232),
            Self::Audio => Color32::from_rgb(118, 196, 148),
            Self::Still => Color32::from_rgb(176, 146, 220),
            Self::Offline => Color32::from_rgb(232, 128, 128),
            Self::Unknown => Color32::from_rgb(152, 152, 160),
        }
    }
}

/// Which ends of a clip use less than the whole source.
///
/// A trimmed edge can be dragged back out, an untrimmed one cannot, so the
/// panel marks the difference: a bar on a trimmed edge, nothing on an edge
/// that is already at the limit of its media.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrimmedEdges {
    /// The clip starts after the beginning of its source.
    pub head: bool,
    /// The clip ends before the end of its source.
    pub tail: bool,
}

impl TrimmedEdges {
    /// Works out which edges of `clip` are trimmed against `media`.
    ///
    /// The tail can only be known when the media has been probed and reported
    /// a duration; an unprobed or endless source reports an untrimmed tail
    /// rather than guessing.
    #[must_use]
    pub fn of(clip: &Clip, media: Option<&MediaItem>) -> Self {
        let start = clip.source_range.start();
        let source_end = media
            .and_then(|media| media.info.as_ref())
            .and_then(|info| info.duration);
        Self {
            head: !start.is_zero() && !start.is_negative(),
            tail: source_end.is_some_and(|end| clip.source_range.end_exclusive() < end),
        }
    }

    /// True if either edge is trimmed.
    #[must_use]
    pub const fn any(self) -> bool {
        self.head || self.tail
    }
}

/// The number of frames between two ruler ticks at least `min_px` points
/// apart.
///
/// The ladder climbs in the units an editor reads time in: single frames,
/// then whole timecode seconds, minutes and hours. Every step is a whole
/// number of frames, so a tick always lands on a frame boundary and its
/// timecode label is exact.
#[must_use]
pub fn tick_interval_frames(rate: Rational, zoom: ZoomLevel, min_px: u32) -> i64 {
    let mut widest = 1;
    for candidate in tick_ladder(rate) {
        widest = candidate;
        if spans_at_least(candidate, zoom, min_px) {
            return candidate;
        }
    }
    // Zoomed out past the top of the ladder: keep doubling the largest step
    // until it is wide enough, which it must become since the zoom range is
    // bounded.
    for _ in 0..64 {
        widest = widest.saturating_mul(2);
        if spans_at_least(widest, zoom, min_px) {
            break;
        }
    }
    widest
}

/// The timecode label for `frame` at `rate`.
///
/// Rates timecode cannot count — anything that is neither a whole number of
/// frames per second nor an NTSC rate — are labelled with the frame number
/// instead, which is still exact.
#[must_use]
pub fn tick_label(frame: i64, rate: Rational) -> String {
    TimecodeRate::new(rate, TimecodeRate::rate_drops_frames(rate)).map_or_else(
        |_| format!("{frame}f"),
        |rate| Timecode::from_frame_number(frame, rate).to_string(),
    )
}

/// The frames at which ticks fall inside `view`, spaced `interval` apart.
///
/// The first tick is the first multiple of `interval` at or after the left
/// edge, so ticks stay anchored to sequence time zero as the view scrolls.
pub fn tick_frames(view: &TimelineView, interval: i64) -> impl Iterator<Item = i64> {
    let visible = view.visible_range();
    let interval = interval.max(1);
    let start = visible.start().value();
    let end = visible.end_exclusive().value();
    let first = start.div_euclid(interval).saturating_mul(interval);
    let first = if first < start {
        first.saturating_add(interval)
    } else {
        first
    };
    core::iter::successors(Some(first), move |frame| {
        Some(frame.saturating_add(interval))
    })
    .take_while(move |frame| *frame <= end)
}

/// The timeline panel's own state: what it is looking at, and the indexes it
/// looks through.
///
/// The [`TrackLayout`] indexes are a cache of the sequence, rebuilt by
/// [`TimelinePanel::sync`] when the engine reports a new revision, never per
/// painted frame.
pub struct TimelinePanel {
    /// The pixel/time mapping and horizontal scroll.
    view: TimelineView,
    /// The fixed sizes the panel lays out with.
    metrics: TimelineMetrics,
    /// One index per track of the synced sequence, in track order.
    layouts: Vec<TrackLayout>,
    /// The revision [`TimelinePanel::layouts`] was built from.
    synced_revision: Option<u64>,
    /// How far the lanes are scrolled down, in points; never negative.
    lane_scroll_px: f32,
    /// The track header column's widgets: the inline rename, and nothing else.
    header_state: TrackHeaderState,
    /// Where the panel's parts sat the last time it was painted.
    last_layout: Option<PanelLayout>,
}

/// What one painted frame of the panel produced.
///
/// The panel mutates nothing itself: the actions are what the header controls
/// asked for, for the caller to turn into commands with
/// [`TrackAction::into_command`] and apply through the Command API.
pub struct TimelineResponse {
    /// The response of the whole panel, for hover and drag tests.
    pub response: Response,
    /// The track actions raised this frame, in the order they were raised.
    pub actions: Vec<TrackAction>,
}

impl TimelinePanel {
    /// A panel looking at the origin of a sequence with timebase `rate`.
    #[must_use]
    pub fn new(rate: Rational) -> Self {
        Self {
            view: TimelineView::new(rate),
            metrics: TimelineMetrics::default(),
            layouts: Vec::new(),
            synced_revision: None,
            lane_scroll_px: 0.0,
            header_state: TrackHeaderState::new(),
            last_layout: None,
        }
    }

    /// Where the panel's parts sat the last time it was painted.
    ///
    /// `None` until the first frame. Hit tests against the header column go
    /// through this rather than guessing at the layout.
    #[must_use]
    pub const fn layout(&self) -> Option<PanelLayout> {
        self.last_layout
    }

    /// The rectangle track `index`'s header occupied when the panel was last
    /// painted, whether or not it was on screen.
    #[must_use]
    pub fn header_rect(&self, index: usize) -> Option<Rect> {
        let layout = self.last_layout?;
        let top = self.lane_top(layout.headers.top(), index);
        Some(Rect::from_min_size(
            pos2(layout.headers.left(), top),
            Vec2::new(layout.headers.width(), self.metrics.track_height),
        ))
    }

    /// The view model behind the panel.
    #[must_use]
    pub const fn view(&self) -> &TimelineView {
        &self.view
    }

    /// The view model, mutably, for the commands that seek and zoom.
    pub const fn view_mut(&mut self) -> &mut TimelineView {
        &mut self.view
    }

    /// The sizes the panel lays out with.
    #[must_use]
    pub const fn metrics(&self) -> &TimelineMetrics {
        &self.metrics
    }

    /// The sizes the panel lays out with, mutably.
    pub const fn metrics_mut(&mut self) -> &mut TimelineMetrics {
        &mut self.metrics
    }

    /// The per-track clip indexes, in track order.
    #[must_use]
    pub fn layouts(&self) -> &[TrackLayout] {
        &self.layouts
    }

    /// How far the lanes are scrolled down, in points.
    #[must_use]
    pub const fn lane_scroll_px(&self) -> f32 {
        self.lane_scroll_px
    }

    /// Rebuilds the clip indexes if `revision` differs from the one they were
    /// built at.
    ///
    /// `revision` is the engine's project revision. Calling this every frame
    /// is intended: it is a comparison until an edit lands, and the panel
    /// paints from a stale index otherwise.
    pub fn sync(&mut self, sequence: &Sequence, revision: u64) {
        let rate = sequence.settings.frame_rate;
        if self.view.rate() != rate {
            let mut view = TimelineView::new(rate);
            view.set_width_px(self.view.width_px());
            view.set_zoom(self.view.zoom());
            self.view = view;
            self.synced_revision = None;
        }
        if self.synced_revision == Some(revision) && self.layouts.len() == sequence.tracks.len() {
            return;
        }
        self.layouts.clear();
        self.layouts.extend(
            sequence
                .tracks
                .iter()
                .map(|track| TrackLayout::build(track, rate)),
        );
        self.synced_revision = Some(revision);
    }

    /// Forgets the clip indexes so the next [`TimelinePanel::sync`] rebuilds
    /// them.
    pub fn invalidate(&mut self) {
        self.synced_revision = None;
    }

    /// The longest track's end: how much sequence there is to scroll through.
    #[must_use]
    pub fn content_duration(&self) -> RationalTime {
        let rate = self.view.rate();
        self.layouts
            .iter()
            .map(|layout| layout.content_duration(rate))
            .max()
            .unwrap_or_else(|| RationalTime::zero(rate))
    }

    /// Applies one wheel or pinch gesture.
    ///
    /// A zoom gesture keeps the instant under the pointer under the pointer;
    /// a scroll gesture moves the lanes. The two never happen together: egui
    /// routes ctrl+wheel to the zoom factor and leaves the scroll delta
    /// alone, and honouring both would move the timeline twice.
    pub fn apply_wheel(&mut self, input: WheelInput, viewport_height: f32, tracks: usize) {
        if input.is_zoom() {
            let (numerator, denominator) = zoom_ratio(input.zoom_factor);
            let zoom = self.view.zoom().scaled(numerator, denominator);
            self.view.zoom_to(zoom, round_px(input.anchor_px));
            return;
        }
        if input.scroll_x != 0.0 {
            self.view.scroll_by(-round_px(input.scroll_x));
        }
        if input.scroll_y != 0.0 {
            self.set_lane_scroll(
                self.lane_scroll_px - input.scroll_y,
                viewport_height,
                tracks,
            );
        }
    }

    /// Puts the lane scroll back to a remembered value, without clamping.
    ///
    /// Restoring a sequence tab happens before egui has told the panel how
    /// tall the lane viewport is this frame, so there is nothing to clamp
    /// against; the next gesture through [`TimelinePanel::set_lane_scroll`]
    /// pulls it back into range. Negative values are pinned to the top.
    pub const fn restore_lane_scroll(&mut self, scroll_px: f32) {
        self.lane_scroll_px = if scroll_px > 0.0 { scroll_px } else { 0.0 };
    }

    /// Scrolls the lanes to `scroll_px`, clamped so at least one lane stays
    /// on screen.
    pub fn set_lane_scroll(&mut self, scroll_px: f32, viewport_height: f32, tracks: usize) {
        let overflow = (self.metrics.lanes_height(tracks) - viewport_height).max(0.0);
        self.lane_scroll_px = scroll_px.clamp(0.0, overflow);
    }

    /// The half-open range of track indexes with a pixel inside a lane
    /// viewport `viewport_height` points tall.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the quotient is clamped to the track count before it is narrowed"
    )]
    pub fn visible_tracks(&self, viewport_height: f32, tracks: usize) -> core::ops::Range<usize> {
        if tracks == 0 || viewport_height <= 0.0 {
            return 0..0;
        }
        let pitch = self.metrics.lane_pitch().max(1.0);
        let first = (self.lane_scroll_px / pitch).floor().max(0.0) as usize;
        let last = ((self.lane_scroll_px + viewport_height) / pitch)
            .ceil()
            .max(0.0) as usize;
        let first = first.min(tracks);
        first..last.saturating_add(1).min(tracks).max(first)
    }

    /// The top of track `index`'s lane, in points from `top`.
    #[allow(
        clippy::cast_precision_loss,
        reason = "a track count large enough to lose precision cannot be painted anyway"
    )]
    fn lane_top(&self, top: f32, index: usize) -> f32 {
        top + index as f32 * self.metrics.lane_pitch() - self.lane_scroll_px
    }

    /// Paints the panel into `ui` and handles its wheel input.
    ///
    /// The panel is read-only: it never mutates `project` or `sequence`, and
    /// every edit still goes through the Command API. Call
    /// [`TimelinePanel::sync`] first so the indexes match the sequence.
    pub fn ui(&mut self, ui: &mut Ui, project: &Project, sequence: &Sequence) -> TimelineResponse {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let layout = PanelLayout::new(rect, &self.metrics);
        self.last_layout = Some(layout);
        self.view.set_width_px(width_px(layout.content.width()));
        let tracks = sequence.tracks.len();
        self.set_lane_scroll(self.lane_scroll_px, layout.content.height(), tracks);
        if response.hovered() {
            let input = wheel_input(ui, &layout);
            self.apply_wheel(input, layout.content.height(), tracks);
        }
        let visuals = ui.visuals().clone();
        let painter = ui.painter().with_clip_rect(rect);
        paint_frame(&painter, &layout, &visuals);
        self.paint_ruler(&painter, &layout, &visuals);
        self.paint_lanes(&painter, &layout, &visuals, project, sequence);
        let actions = self.header_controls(ui, &layout, sequence);
        TimelineResponse { response, actions }
    }

    /// Runs the header column's controls over the visible tracks.
    ///
    /// The controls are widgets rather than painted shapes — see
    /// [`crate::track_header`] — and they are the only interactive part of the
    /// panel, so they are laid out after everything is painted, on top of the
    /// header backgrounds.
    fn header_controls(
        &mut self,
        ui: &mut Ui,
        layout: &PanelLayout,
        sequence: &Sequence,
    ) -> Vec<TrackAction> {
        let mut actions = Vec::new();
        let count = sequence.tracks.len();
        let mut column = ui.new_child(
            eframe::egui::UiBuilder::new()
                .max_rect(layout.headers)
                .id_salt("track_headers"),
        );
        column.set_clip_rect(layout.headers);
        let mut filled_to = layout.headers.top();
        for index in self.visible_tracks(layout.headers.height(), count) {
            let Some(track) = sequence.tracks.get(index) else {
                continue;
            };
            let top = self.lane_top(layout.headers.top(), index);
            let rect = Rect::from_min_size(
                pos2(layout.headers.left(), top),
                Vec2::new(layout.headers.width(), self.metrics.track_height),
            );
            filled_to = filled_to.max(rect.bottom());
            if let Some(action) = self.header_state.ui(&mut column, rect, track, index, count) {
                actions.push(action);
            }
        }
        let spare = Rect::from_min_max(
            pos2(layout.headers.left(), filled_to.max(layout.headers.top())),
            layout.headers.max,
        );
        if spare.height() > 1.0 {
            let response = column.interact(
                spare,
                column.id().with("empty_column"),
                Sense::click_and_drag(),
            );
            if let Some(action) = empty_column_menu(&response, count) {
                actions.push(action);
            }
        }
        actions
    }

    /// Paints the timecode ruler and the grid lines that drop from it.
    fn paint_ruler(
        &self,
        painter: &eframe::egui::Painter,
        layout: &PanelLayout,
        visuals: &Visuals,
    ) {
        let rate = self.view.rate();
        let zoom = self.view.zoom();
        let minor = tick_interval_frames(rate, zoom, MIN_TICK_SPACING_PX);
        let major = tick_interval_frames(rate, zoom, MIN_LABEL_SPACING_PX);
        let ruler = painter.with_clip_rect(layout.ruler);
        let faint = visuals.weak_text_color();
        for frame in tick_frames(&self.view, minor) {
            if frame % major == 0 {
                continue;
            }
            let x = layout.content.left() + self.view.pixel_of(RationalTime::new(frame, rate));
            ruler.line_segment(
                [
                    pos2(x, layout.ruler.bottom() - 5.0),
                    pos2(x, layout.ruler.bottom()),
                ],
                Stroke::new(1.0, faint),
            );
        }
        let grid = painter.with_clip_rect(layout.content);
        for frame in tick_frames(&self.view, major) {
            let x = layout.content.left() + self.view.pixel_of(RationalTime::new(frame, rate));
            ruler.line_segment(
                [
                    pos2(x, layout.ruler.top() + 3.0),
                    pos2(x, layout.ruler.bottom()),
                ],
                Stroke::new(1.0, visuals.text_color()),
            );
            ruler.text(
                pos2(x + 4.0, layout.ruler.center().y),
                Align2::LEFT_CENTER,
                tick_label(frame, rate),
                FontId::monospace(11.0),
                visuals.text_color(),
            );
            grid.line_segment(
                [
                    pos2(x, layout.content.top()),
                    pos2(x, layout.content.bottom()),
                ],
                Stroke::new(1.0, visuals.faint_bg_color),
            );
        }
    }

    /// Paints the track headers and the clips in their lanes, top-down.
    fn paint_lanes(
        &self,
        painter: &eframe::egui::Painter,
        layout: &PanelLayout,
        visuals: &Visuals,
        project: &Project,
        sequence: &Sequence,
    ) {
        let lanes = painter.with_clip_rect(layout.content);
        let headers = painter.with_clip_rect(layout.headers);
        let range = self.visible_tracks(layout.content.height(), sequence.tracks.len());
        for index in range {
            let Some(track) = sequence.tracks.get(index) else {
                continue;
            };
            let top = self.lane_top(layout.content.top(), index);
            let lane = Rect::from_min_size(
                pos2(layout.content.left(), top),
                Vec2::new(layout.content.width(), self.metrics.track_height),
            );
            lanes.rect_filled(lane, CornerRadius::ZERO, lane_color(visuals, track));
            paint_header(
                &headers,
                Rect::from_min_size(
                    pos2(layout.headers.left(), top),
                    Vec2::new(layout.headers.width(), self.metrics.track_height),
                ),
                visuals,
            );
            if let Some(index) = self.layouts.get(index) {
                self.paint_clips(&lanes, lane, index, track, project);
            }
        }
    }

    /// Paints the clips of one track that the viewport touches.
    fn paint_clips(
        &self,
        painter: &eframe::egui::Painter,
        lane: Rect,
        index: &TrackLayout,
        track: &Track,
        project: &Project,
    ) {
        let body = lane.shrink2(Vec2::new(0.0, 3.0));
        for placement in self.view.visible_clips(index) {
            let Some(clip) = track
                .items
                .get(placement.item_index)
                .and_then(sub_model::TrackItem::as_clip)
            else {
                continue;
            };
            let left = lane.left() + self.view.pixel_of(placement.range.start());
            let right = lane.left() + self.view.pixel_of(placement.range.end_exclusive());
            let rect = Rect::from_min_max(
                pos2(left, body.top()),
                pos2(right.max(left + 1.0), body.bottom()),
            );
            let media = project.media_item(clip.media);
            paint_clip(
                painter,
                rect,
                clip,
                ClipMediaKind::of(media),
                TrimmedEdges::of(clip, media),
                !clip_edits_allowed(track),
            );
        }
    }
}

/// Paints one clip rectangle: its body, its outline, its trimmed edges and its
/// name.
fn paint_clip(
    painter: &eframe::egui::Painter,
    rect: Rect,
    clip: &Clip,
    kind: ClipMediaKind,
    trimmed: TrimmedEdges,
    dimmed: bool,
) {
    let radius = CornerRadius::same(3);
    let fill = dim(kind.fill(), dimmed);
    let outline = dim(kind.outline(), dimmed);
    painter.rect_filled(rect, radius, fill);
    painter.rect_stroke(rect, radius, Stroke::new(1.0, outline), StrokeKind::Inside);
    if trimmed.head {
        painter.rect_filled(
            Rect::from_min_max(rect.min, pos2(rect.left() + TRIM_BAR_WIDTH, rect.bottom())),
            CornerRadius::ZERO,
            outline,
        );
    }
    if trimmed.tail {
        painter.rect_filled(
            Rect::from_min_max(pos2(rect.right() - TRIM_BAR_WIDTH, rect.top()), rect.max),
            CornerRadius::ZERO,
            outline,
        );
    }
    if rect.width() >= MIN_NAME_WIDTH_PX && !clip.name.is_empty() {
        painter.with_clip_rect(rect).text(
            pos2(rect.left() + TRIM_BAR_WIDTH + 3.0, rect.top() + 2.0),
            Align2::LEFT_TOP,
            &clip.name,
            FontId::proportional(11.0),
            dim(Color32::from_gray(235), dimmed),
        );
    }
}

/// Whether the clips on `track` may be edited.
///
/// A locked track refuses every clip command with `edit.track_locked`
/// (see `sub_edit::commands::track_for_clip_edit`), so the panel neither
/// offers a clip edit on one nor paints its clips at full strength.
#[must_use]
pub const fn clip_edits_allowed(track: &Track) -> bool {
    !track.locked
}

/// `color`, dimmed when it belongs to a locked track.
#[must_use]
fn dim(color: Color32, dimmed: bool) -> Color32 {
    if dimmed {
        color.gamma_multiply(LOCKED_DIM)
    } else {
        color
    }
}

/// Paints the plate one track header's controls sit on.
///
/// The name, the kind and the mute and lock toggles are widgets, painted over
/// this by [`TrackHeaderState::ui`](crate::track_header::TrackHeaderState::ui),
/// because a header is worth an id and a layout pass in a way a clip rectangle
/// is not.
fn paint_header(painter: &eframe::egui::Painter, rect: Rect, visuals: &Visuals) {
    painter.rect_filled(rect, CornerRadius::same(2), visuals.extreme_bg_color);
}

/// Paints the panel's backgrounds and the two lines dividing its areas.
fn paint_frame(painter: &eframe::egui::Painter, layout: &PanelLayout, visuals: &Visuals) {
    painter.rect_filled(layout.rect, CornerRadius::ZERO, visuals.panel_fill);
    painter.rect_filled(layout.ruler, CornerRadius::ZERO, visuals.faint_bg_color);
    painter.rect_filled(layout.corner, CornerRadius::ZERO, visuals.faint_bg_color);
    let stroke = Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color);
    painter.line_segment(
        [
            pos2(layout.content.left(), layout.rect.top()),
            pos2(layout.content.left(), layout.rect.bottom()),
        ],
        stroke,
    );
    painter.line_segment(
        [
            pos2(layout.rect.left(), layout.content.top()),
            pos2(layout.rect.right(), layout.content.top()),
        ],
        stroke,
    );
}

/// The lane background of `track`: dimmer for audio, dimmer again when muted.
fn lane_color(visuals: &Visuals, track: &Track) -> Color32 {
    let base = match track.kind {
        TrackKind::Video => visuals.extreme_bg_color,
        TrackKind::Audio => visuals.faint_bg_color,
    };
    let base = if track.muted {
        base.gamma_multiply(0.6)
    } else {
        base
    };
    dim(base, !clip_edits_allowed(track))
}

/// Reads this frame's wheel and pinch input, anchored at the pointer.
fn wheel_input(ui: &Ui, layout: &PanelLayout) -> WheelInput {
    let anchor_default = layout.content.width() / 2.0;
    ui.ctx().input(|input| {
        let anchor_px = input
            .pointer
            .hover_pos()
            .map_or(anchor_default, |pos| pos.x - layout.content.left());
        WheelInput {
            zoom_factor: input.zoom_delta(),
            scroll_x: input.smooth_scroll_delta.x,
            scroll_y: input.smooth_scroll_delta.y,
            anchor_px,
        }
    })
}

/// The zoom factor of a gesture as an exact fraction.
///
/// The factor arrives from the pointing device as a float and is the only
/// float in the zoom path; it is turned into a fraction here so that the zoom
/// itself, and every time derived from it, stays exact.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the factor is clamped to a small positive range before it is narrowed"
)]
fn zoom_ratio(factor: f32) -> (u32, u32) {
    let clamped = factor.clamp(1.0 / 16.0, 16.0);
    let numerator = (clamped * f32::from(u16::try_from(ZOOM_RATIO_DENOMINATOR).unwrap_or(1)))
        .round()
        .max(1.0) as u32;
    (numerator, ZOOM_RATIO_DENOMINATOR)
}

/// A width in points as a whole number of pixels.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a negative or absurd width is clamped before it is narrowed"
)]
fn width_px(width: f32) -> u32 {
    width.clamp(0.0, f32::from(u16::MAX)) as u32
}

/// A scroll delta in points as a whole number of pixels.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a scroll delta larger than the viewport is clamped before it is narrowed"
)]
fn round_px(delta: f32) -> i64 {
    delta.clamp(-1.0e9, 1.0e9).round() as i64
}

/// True if `frames` frames span at least `min_px` points at `zoom`.
fn spans_at_least(frames: i64, zoom: ZoomLevel, min_px: u32) -> bool {
    let pixels_per_frame = zoom.pixels_per_frame();
    i128::from(frames) * i128::from(pixels_per_frame.numerator())
        >= i128::from(min_px) * i128::from(pixels_per_frame.denominator())
}

/// The tick ladder for `rate`, in frames, from finest to coarsest.
///
/// Steps below a second are frame counts that divide the rate exactly, so a
/// tick never drifts within the second; the rest are whole numbers of
/// timecode seconds, so labels land on second, minute and hour boundaries.
fn tick_ladder(rate: Rational) -> impl Iterator<Item = i64> {
    let fps = nominal_fps(rate);
    let frames = [1, 2, 3, 4, 5, 6, 8, 10, 12, 15, 20, 30]
        .into_iter()
        .filter(move |step| *step < fps && fps % *step == 0);
    let seconds = [1, 2, 5, 10, 15, 30]
        .into_iter()
        .map(move |step| step * fps);
    let minutes = [1, 2, 5, 10, 15, 30]
        .into_iter()
        .map(move |step| step * 60 * fps);
    let hours = [1, 2, 4, 6, 12, 24]
        .into_iter()
        .map(move |step| step * 3600 * fps);
    frames.chain(seconds).chain(minutes).chain(hours)
}

/// The whole number of frames in one timecode second at `rate`.
fn nominal_fps(rate: Rational) -> i64 {
    let numerator = i64::from(rate.numerator());
    let denominator = i64::from(rate.denominator());
    ((numerator + denominator / 2) / denominator).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sub_model::media::{AudioStream, StreamInfo, VideoStream};
    use sub_model::sequence::SequenceSettings;
    use sub_model::{ColorTags, MediaPath, TrackItem};
    use sub_time::TimeRange;

    const RATE: Rational = Rational::FPS_24;

    fn frames(value: i64) -> RationalTime {
        RationalTime::new(value, RATE)
    }

    fn range(start: i64, duration: i64) -> TimeRange {
        TimeRange::new(frames(start), frames(duration)).expect("valid range")
    }

    fn zoom(numerator: u32, denominator: u32) -> ZoomLevel {
        ZoomLevel::new(Rational::new(numerator, denominator).expect("valid scale"))
            .expect("in range")
    }

    fn media(kind: ClipMediaKind, duration: Option<i64>) -> MediaItem {
        let mut item = MediaItem::new(MediaPath::new("clip.mp4").expect("valid path"));
        let video = VideoStream {
            width: 1920,
            height: 1080,
            frame_rate: RATE,
            sample_aspect: Rational::ONE,
            color: ColorTags::REC709,
        };
        let audio = AudioStream {
            channels: 2,
            sample_rate: 48_000,
        };
        let mut info = StreamInfo {
            duration: duration.map(frames),
            ..StreamInfo::default()
        };
        match kind {
            ClipMediaKind::Video => {
                info.video.push(video);
                info.audio.push(audio);
            }
            ClipMediaKind::Still => info.video.push(video),
            ClipMediaKind::Audio => info.audio.push(audio),
            ClipMediaKind::Offline => item.offline = true,
            ClipMediaKind::Unknown => {}
        }
        if kind != ClipMediaKind::Unknown {
            item.info = Some(info);
        }
        item
    }

    /// A project holding one video media item lasting `duration` frames, and a
    /// sequence whose one track carries `count` two-second clips.
    fn project_with(count: usize, source_duration: i64) -> (Project, Sequence) {
        let mut project = Project::new("test");
        let item = media(ClipMediaKind::Video, Some(source_duration));
        let media_id = item.id;
        project.media.push(item);
        let mut track = Track::new("V1", TrackKind::Video);
        for index in 0..count {
            track.items.push(TrackItem::Clip(Clip::new(
                format!("clip {index}"),
                media_id,
                range(0, 48),
            )));
        }
        let mut sequence = Sequence::new("seq", SequenceSettings::default());
        sequence.tracks.push(track);
        (project, sequence)
    }

    #[test]
    fn tick_intervals_climb_from_frames_to_seconds_and_minutes() {
        // Eight pixels a frame: single frames are too tight for a label, so
        // the ladder settles on the half-second, twelve frames at 24 fps.
        assert_eq!(
            tick_interval_frames(RATE, zoom(8, 1), MIN_LABEL_SPACING_PX),
            12
        );
        // Four pixels a frame leaves room for exactly one second per label.
        assert_eq!(
            tick_interval_frames(RATE, zoom(4, 1), MIN_LABEL_SPACING_PX),
            24
        );
        // Two pixels a frame needs two seconds.
        assert_eq!(
            tick_interval_frames(RATE, zoom(2, 1), MIN_LABEL_SPACING_PX),
            48
        );
        // Every sub-second step divides the rate, so a tick never drifts
        // within the second.
        assert!(
            tick_ladder(RATE)
                .take_while(|step| *step < 24)
                .all(|step| 24 % step == 0)
        );
        // The same zoom leaves plenty of room for unlabelled ticks every two
        // frames.
        assert_eq!(
            tick_interval_frames(RATE, zoom(8, 1), MIN_TICK_SPACING_PX),
            2
        );
        // Far out: one pixel covers 64 frames, so labels need five minutes.
        assert_eq!(
            tick_interval_frames(RATE, zoom(1, 64), MIN_LABEL_SPACING_PX),
            5 * 60 * 24
        );
        // Every interval is a whole number of frames wide enough to hold a
        // label, at every rung of the zoom ladder.
        for steps in -20..=10 {
            let zoom = ZoomLevel::ONE.zoomed(steps);
            let interval = tick_interval_frames(RATE, zoom, MIN_LABEL_SPACING_PX);
            assert!(interval >= 1, "interval is a positive frame count");
            assert!(
                spans_at_least(interval, zoom, MIN_LABEL_SPACING_PX),
                "interval {interval} is too narrow to label at {steps} steps"
            );
        }
    }

    #[test]
    fn ticks_are_anchored_to_sequence_zero_and_labelled_as_timecode() {
        let mut view = TimelineView::new(RATE);
        view.set_width_px(960);
        view.set_zoom(zoom(4, 1));
        view.scroll_to(frames(30));
        let ticks: Vec<i64> = tick_frames(&view, 24).collect();
        assert_eq!(ticks.first().copied(), Some(48), "first whole second shown");
        assert!(ticks.windows(2).all(|pair| pair[1] - pair[0] == 24));
        assert_eq!(tick_label(0, RATE), "00:00:00:00");
        assert_eq!(tick_label(24 * 61 + 5, RATE), "00:01:01:05");
        // A rate timecode cannot count still gets an exact label.
        let odd = Rational::new(7, 2).expect("valid rate");
        assert_eq!(tick_label(9, odd), "9f");
    }

    #[test]
    fn media_kind_colours_clips_and_flags_offline_media() {
        assert_eq!(ClipMediaKind::of(None), ClipMediaKind::Unknown);
        for kind in [
            ClipMediaKind::Video,
            ClipMediaKind::Audio,
            ClipMediaKind::Still,
            ClipMediaKind::Offline,
            ClipMediaKind::Unknown,
        ] {
            let duration = if kind == ClipMediaKind::Still {
                None
            } else {
                Some(240)
            };
            assert_eq!(
                ClipMediaKind::of(Some(&media(kind, duration))),
                kind,
                "{} media should classify as itself",
                kind.label()
            );
        }
        let fills: std::collections::BTreeSet<[u8; 4]> = [
            ClipMediaKind::Video,
            ClipMediaKind::Audio,
            ClipMediaKind::Still,
            ClipMediaKind::Offline,
            ClipMediaKind::Unknown,
        ]
        .into_iter()
        .map(|kind| kind.fill().to_array())
        .collect();
        assert_eq!(fills.len(), 5, "every media kind has its own colour");
    }

    #[test]
    fn trimmed_edges_follow_the_source_range() {
        let source = media(ClipMediaKind::Video, Some(240));
        let untrimmed = Clip::new("a", source.id, range(0, 240));
        assert_eq!(
            TrimmedEdges::of(&untrimmed, Some(&source)),
            TrimmedEdges::default()
        );
        assert!(!TrimmedEdges::of(&untrimmed, Some(&source)).any());

        let both = Clip::new("b", source.id, range(24, 100));
        assert_eq!(
            TrimmedEdges::of(&both, Some(&source)),
            TrimmedEdges {
                head: true,
                tail: true
            }
        );

        let head_only = Clip::new("c", source.id, range(24, 216));
        assert_eq!(
            TrimmedEdges::of(&head_only, Some(&source)),
            TrimmedEdges {
                head: true,
                tail: false
            }
        );

        // An unprobed source cannot prove the tail is trimmed.
        let unknown = media(ClipMediaKind::Unknown, None);
        assert_eq!(
            TrimmedEdges::of(&both, Some(&unknown)),
            TrimmedEdges {
                head: true,
                tail: false
            }
        );
    }

    #[test]
    fn ctrl_wheel_zooms_around_the_pointer_and_the_wheel_scrolls() {
        let mut panel = TimelinePanel::new(RATE);
        panel.view_mut().set_width_px(1000);
        panel.view_mut().set_zoom(zoom(4, 1));
        panel.view_mut().scroll_to(frames(100));
        let under_pointer = panel.view().time_at_pixel(300);

        panel.apply_wheel(
            WheelInput {
                zoom_factor: 2.0,
                scroll_x: 0.0,
                scroll_y: 0.0,
                anchor_px: 300.0,
            },
            500.0,
            1,
        );
        assert_eq!(
            panel.view().zoom(),
            zoom(8, 1),
            "a doubling factor doubles the zoom"
        );
        assert_eq!(
            panel.view().time_at_pixel(300),
            under_pointer,
            "the frame under the pointer stays there"
        );

        // A plain wheel scrolls instead: content moving left scrolls forward.
        let before = panel.view().scroll_px();
        panel.apply_wheel(
            WheelInput {
                scroll_x: -40.0,
                ..WheelInput::none(300.0)
            },
            500.0,
            1,
        );
        assert_eq!(panel.view().scroll_px(), before + 40);
        assert_eq!(
            panel.view().zoom(),
            zoom(8, 1),
            "scrolling never changes zoom"
        );
    }

    #[test]
    fn lane_scrolling_stops_at_the_ends() {
        let mut panel = TimelinePanel::new(RATE);
        let viewport = 100.0;
        panel.apply_wheel(
            WheelInput {
                scroll_y: -1000.0,
                ..WheelInput::none(0.0)
            },
            viewport,
            8,
        );
        let overflow = panel.metrics().lanes_height(8) - viewport;
        assert!((panel.lane_scroll_px() - overflow).abs() < f32::EPSILON);
        panel.apply_wheel(
            WheelInput {
                scroll_y: 1000.0,
                ..WheelInput::none(0.0)
            },
            viewport,
            8,
        );
        assert!(
            panel.lane_scroll_px().abs() < f32::EPSILON,
            "never scrolls above the first lane"
        );
        // Only the lanes with a pixel on screen are painted.
        let visible = panel.visible_tracks(viewport, 8);
        assert!(visible.len() < 8, "off-screen lanes are skipped");
    }

    #[test]
    fn syncing_rebuilds_the_indexes_only_when_the_revision_changes() {
        let (_, sequence) = project_with(4, 240);
        let mut panel = TimelinePanel::new(RATE);
        panel.sync(&sequence, 1);
        assert_eq!(panel.layouts().len(), 1);
        assert_eq!(panel.layouts()[0].len(), 4);
        assert_eq!(panel.content_duration(), frames(4 * 48));

        let mut edited = sequence.clone();
        edited.tracks[0].items.pop();
        panel.sync(&edited, 1);
        assert_eq!(
            panel.layouts()[0].len(),
            4,
            "a stale revision keeps the index"
        );
        panel.sync(&edited, 2);
        assert_eq!(panel.layouts()[0].len(), 3, "a new revision rebuilds it");
    }
}
