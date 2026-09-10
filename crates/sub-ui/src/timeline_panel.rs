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
    Align2, Color32, Context, CornerRadius, CursorIcon, FontId, Key, Painter, Pos2, Rect, Response,
    Sense, Stroke, StrokeKind, TextEdit, TextStyle, Ui, UiBuilder, Vec2, Visuals, pos2,
};
use sub_model::{Clip, Marker, MarkerId, MediaItem, Project, Sequence, Track, TrackKind};
use sub_time::{Rational, RationalTime, TimeRange, Timecode, TimecodeRate};

use crate::fade::{FadeEdge, FadeEdit, FadeRefusal, plan_fade};
use crate::markers::{self, DEFAULT_MARKER_NAME, MarkerAction, MarkerState};
use crate::media_bin::BinDrag;
use crate::selection::{ClipRef, MoveGroup, MoveRefusal, Selection, clips_in_marquee, plan_move};
use crate::snapping::{self, SnapCandidate, SnapKind, SnapSettings};
use crate::source_edit::{EditMode, PlannedEdit, SourceRefusal, plan_source_edit};
use crate::split::{SplitGroup, SplitRefusal, plan_split, plan_split_clip};
use crate::thumbnails::{ThumbnailCache, ZoomBucket, tile_time};
use crate::timeline::{TimelineView, TrackLayout, ZoomLevel};
use crate::track_header::{TrackAction, TrackHeaderState, empty_column_menu};
use crate::trim::{TrimEdge, TrimGroup, TrimRefusal, plan_trim};
use crate::waveform::WaveformCache;

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

/// How far into a clip a press still counts as grabbing its edge, in points.
///
/// Wide enough to aim at with a mouse and narrow enough that the middle of
/// even a short clip still starts a move; a clip narrower than twice this
/// splits its width between the two handles rather than letting them overlap.
pub const TRIM_HANDLE_PX: f32 = 6.0;

/// How much of [`SELECTION_COLOR`] a trim handle is painted in at rest.
const TRIM_HANDLE_ALPHA: u8 = 140;

/// The narrowest clip that has room to be painted with trim handles.
const MIN_HANDLE_WIDTH_PX: f32 = 3.0 * TRIM_HANDLE_PX;

/// How far from a fade handle a press still counts as grabbing it, in points.
pub const FADE_HANDLE_PX: f32 = 6.0;

/// How far down an audio clip the fade handles reach, in points.
///
/// The band is the top of the clip so that the rest of it, waveform and all,
/// still starts a move; below the band a press near an edge is a trim, as it
/// is on any other clip.
pub const FADE_BAND_PX: f32 = 12.0;

/// The size of the square drawn at a fade handle, in points.
const FADE_GRIP_PX: f32 = 5.0;

/// The narrowest audio clip that has room for two fade handles.
const MIN_FADE_WIDTH_PX: f32 = 3.0 * FADE_HANDLE_PX;

/// How much of [`FADE_COLOR`] a fade ramp is drawn in at rest.
pub const FADE_ALPHA: u8 = 170;

/// The colour a fade ramp and its handles are drawn in.
pub const FADE_COLOR: Color32 = Color32::from_rgb(240, 236, 214);

/// How far a waveform strip is inset from the top and bottom of its clip, in
/// points, so the clip's name and outline stay readable over it.
const WAVEFORM_INSET: f32 = 2.0;

/// The narrowest clip rectangle worth drawing a waveform in.
const MIN_WAVEFORM_WIDTH_PX: f32 = 3.0;

/// How much of the clip's outline colour a waveform strip is drawn in.
const WAVEFORM_ALPHA: u8 = 190;

/// How much of its colour a clip on a locked track keeps.
///
/// A locked track refuses clip edits (`edit.track_locked`), and the lane has
/// to say so before the edit is attempted rather than after.
const LOCKED_DIM: f32 = 0.45;

/// The narrowest clip rectangle that is worth painting a thumbnail strip in.
///
/// Below this a tile would be a few pixels of a picture, which says less than
/// the clip's own colour does.
const MIN_STRIP_WIDTH_PX: f32 = 18.0;

/// The shortest lane a thumbnail strip is painted in.
const MIN_STRIP_HEIGHT_PX: f32 = 12.0;

/// The narrowest one thumbnail tile may be, in points.
const MIN_TILE_WIDTH_PX: f32 = 8.0;

/// The most tiles one clip's strip is painted as.
///
/// A very long clip at a very close zoom would otherwise ask for a tile per
/// few pixels across the whole viewport; past this the tiles are stretched
/// instead, which costs a bounded number of textures per clip.
pub const MAX_TILES_PER_CLIP: usize = 64;

/// How dark the plate behind a clip name is when a strip is painted under it.
const NAME_PLATE_ALPHA: u8 = 150;

/// How far from 1.0 a zoom factor has to be before it counts as a gesture.
const ZOOM_EPSILON: f32 = 0.001;

/// The colour the playhead is drawn in.
///
/// Deliberately outside the clip palette above — no clip is this colour — so
/// the playhead reads as the one thing on the timeline that is not part of
/// the edit.
const PLAYHEAD_COLOR: Color32 = Color32::from_rgb(236, 84, 72);

/// How wide the playhead line is, in points.
const PLAYHEAD_WIDTH: f32 = 1.0;

/// How wide the playhead's head in the ruler is, in points.
const PLAYHEAD_HEAD_WIDTH: f32 = 9.0;

/// How tall the playhead's head in the ruler is, in points.
const PLAYHEAD_HEAD_HEIGHT: f32 = 7.0;

/// The colour a snapped edge is flagged in while a scrub is landing on it.
const SNAP_COLOR: Color32 = Color32::from_rgb(240, 208, 96);

/// How wide a marker's flag on the ruler is, in points.
///
/// Wide enough to aim a pointer at without covering the timecode label that
/// may sit beside it.
pub const MARKER_FLAG_WIDTH: f32 = 9.0;

/// How tall a marker's flag on the ruler is, in points.
///
/// The flag hangs from the bottom of the ruler, so it never collides with the
/// playhead's head, which sits at the top.
pub const MARKER_FLAG_HEIGHT: f32 = 11.0;

/// How far either side of a marker's flag still counts as grabbing it, in
/// points.
const MARKER_GRAB_SLACK: f32 = 3.0;

/// How much of its colour a marker's guide line down the lanes keeps.
///
/// Faint on purpose: a marker annotates the edit rather than being part of
/// it, so its line must not read as a cut.
const MARKER_GUIDE_ALPHA: u8 = 70;

/// How wide the inline rename editor over a marker is, in points.
const MARKER_RENAME_WIDTH: f32 = 110.0;

/// The gap between a marker's flag and the name painted beside it, in points.
const MARKER_NAME_GAP: f32 = 3.0;

/// The colour a marker's name is painted in, over the ruler.
const MARKER_NAME_COLOR: Color32 = Color32::from_rgb(226, 226, 230);

/// How thick the outline around the selected marker's flag is, in points.
const MARKER_SELECTED_WIDTH: f32 = 1.5;

/// How tall the bar under a span marker is, in points.
const MARKER_SPAN_HEIGHT: f32 = 3.0;

/// The denominator a wheel zoom factor is approximated over.
const ZOOM_RATIO_DENOMINATOR: u32 = 4096;

/// The colour a selected clip is outlined in.
///
/// Near-white, which is the one thing brighter than every clip fill and
/// outline in the palette above, so selection reads at a glance whatever kind
/// of media is under it — and it is what every other editor draws.
const SELECTION_COLOR: Color32 = Color32::from_rgb(248, 248, 252);

/// How wide the outline round a selected clip is, in points.
const SELECTION_WIDTH: f32 = 2.0;

/// The colour a drag that cannot become an edit is painted in.
const REFUSED_COLOR: Color32 = Color32::from_rgb(224, 88, 88);

/// How much of [`REFUSED_COLOR`] is washed over a clip whose drag is refused.
const REFUSED_ALPHA: u8 = 64;

/// How much of [`SELECTION_COLOR`] fills a ghost of a clip being dragged.
const GHOST_ALPHA: u8 = 56;

/// How much of [`SELECTION_COLOR`] fills the marquee rectangle.
const MARQUEE_ALPHA: u8 = 32;

/// The colour the razor's cut line is drawn in.
///
/// The same yellow a snapped edge is flagged in, because it is the same
/// promise: this is the exact instant the gesture will land on.
const RAZOR_COLOR: Color32 = SNAP_COLOR;

/// How wide the razor's cut line is, in points.
const RAZOR_WIDTH: f32 = 1.0;

/// How tall the blade drawn at the head of the razor's line is, in points.
const RAZOR_BLADE_HEIGHT: f32 = 7.0;

/// How wide the blade drawn at the head of the razor's line is, in points.
const RAZOR_BLADE_WIDTH: f32 = 7.0;

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

/// How one clip's thumbnail strip is divided into tiles.
///
/// Tiles keep the thumbnail's own aspect, so a picture is never stretched,
/// until a clip is long enough to want more than [`MAX_TILES_PER_CLIP`] of
/// them; from there the tiles widen rather than multiply.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripTiles {
    /// How wide one tile is, in points.
    pub tile_width: f32,
    /// How many tiles cover the clip. The last one is usually cut short by
    /// the clip's right edge.
    pub count: usize,
}

/// The tiles a clip `width` by `height` points shows of a thumbnail written
/// `thumb_width` by `thumb_height` pixels.
///
/// Returns no tiles for a clip too small to say anything with a picture.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "tile counts are clamped into 1..=MAX_TILES_PER_CLIP before they are narrowed"
)]
pub fn strip_tiles(width: f32, height: f32, thumb_width: u32, thumb_height: u32) -> StripTiles {
    let none = StripTiles {
        tile_width: 0.0,
        count: 0,
    };
    if !width.is_finite()
        || !height.is_finite()
        || width < MIN_STRIP_WIDTH_PX
        || height < MIN_STRIP_HEIGHT_PX
    {
        return none;
    }
    let aspect = if thumb_width == 0 || thumb_height == 0 {
        16.0 / 9.0
    } else {
        thumb_width as f32 / thumb_height as f32
    };
    let mut tile_width = (height * aspect).max(MIN_TILE_WIDTH_PX);
    let mut count = ((width / tile_width).ceil().max(1.0) as usize).min(MAX_TILES_PER_CLIP);
    if count == MAX_TILES_PER_CLIP {
        tile_width = width / count as f32;
    }
    count = count.max(1);
    StripTiles { tile_width, count }
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

/// What a press in the lanes means.
///
/// A tool is view state, not part of the edit: it decides which gesture a
/// press starts, and nothing else. The two the editor has are the arrow —
/// select, marquee and drag — and the razor, which cuts the clip it is
/// clicked on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tool {
    /// Select clips, band-select and drag them: the default.
    #[default]
    Select,
    /// Cut the clicked clip at the instant clicked.
    Razor,
}

impl Tool {
    /// A stable identifier, for logging and for tests.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Razor => "razor",
        }
    }

    /// What the tool is called in the interface.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Razor => "Razor",
        }
    }
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
    /// The waveform peaks and textures the audio clips are drawn with.
    waveforms: WaveformCache,
    /// Where the panel's parts sat the last time it was painted.
    last_layout: Option<PanelLayout>,
    /// The thumbnail textures the clip strips are painted from.
    thumbnails: ThumbnailCache,
    /// Where the playhead is, in sequence time.
    ///
    /// The playhead is where the user is looking, not part of the edit, so it
    /// is never project state and moving it is never a Command
    /// (`sub_edit::playback`). The application owns the one true playhead in
    /// the viewer and hands it to the panel each frame with
    /// [`TimelinePanel::set_playhead`]; a click on the ruler asks for a new
    /// one through [`TimelineResponse::seek`].
    playhead: RationalTime,
    /// Whether a time dropped on the timeline is pulled onto a snap target,
    /// and how close counts.
    snap: SnapSettings,
    /// Whether a scrub started in the ruler is still under the pointer.
    scrubbing: bool,
    /// The snap targets gathered for the scrub in progress.
    ///
    /// Kept between frames so a scrub, which needs them every frame, does not
    /// allocate after its first.
    candidates: Vec<SnapCandidate>,
    /// What the last reported seek snapped to, for painting the flag.
    snapped: Option<SnapCandidate>,
    /// The clips the editor has selected.
    ///
    /// Selection is what the editor is pointing at rather than part of the
    /// edit, so it lives here and never in the project; only the move it
    /// produces is a Command (see [`crate::selection`]).
    selection: Selection,
    /// The gesture in the lanes that is still under the pointer.
    gesture: Option<Gesture>,
    /// What was selected when a marquee started, so the band adds to it.
    marquee_base: Vec<ClipRef>,
    /// What the drag in progress would do, recomputed each frame.
    ///
    /// `Ok` is the move that will be committed on release and the ghosts that
    /// preview it; `Err` is why it will not be, which is what makes a refused
    /// drag visible before the button comes up.
    drag_plan: Option<Result<MoveGroup, MoveRefusal>>,
    /// What the trim drag in progress would do, recomputed each frame.
    ///
    /// The same shape as [`TimelinePanel::drag_plan`], and for the same
    /// reason: `Ok` is the ghost and the commands, `Err` is why there are
    /// none.
    trim_plan: Option<Result<TrimGroup, TrimRefusal>>,
    /// The clip edge under the pointer, which is what turns the cursor into a
    /// trim cursor and paints the handle bright.
    hovered_trim: Option<(ClipRef, TrimEdge)>,
    /// What the fade drag in progress would do, recomputed each frame, so the
    /// ramp on screen is the fade the release will commit.
    fade_plan: Option<Result<FadeEdit, FadeRefusal>>,
    /// The fade handle under the pointer, which is painted bright.
    hovered_fade: Option<(ClipRef, FadeEdge)>,
    /// The ruler's markers: what is selected, dragged or being renamed.
    marker_state: MarkerState,
    /// Whether the pointer press in progress was claimed by a marker, in
    /// which case it is a marker gesture and not a scrub.
    marker_press: bool,
    /// How far into the flag the marker being dragged was grabbed, in points.
    ///
    /// A flag is wide enough to aim at, so without this a marker would jump
    /// by however far from its head the pointer came down. Pixels rather than
    /// time on purpose: it is a property of the grip, not of the edit, and it
    /// stays the same width as the view zooms.
    marker_grab_offset: f32,
    /// Marker actions raised from outside a painted frame — the `M` shortcut
    /// — waiting for the next frame to report them.
    pending_markers: Vec<MarkerAction>,
    /// What a press in the lanes means: select, or cut.
    tool: Tool,
    /// Where the razor would cut if it were clicked now, snapped.
    ///
    /// Recomputed from the pointer every frame the razor is out, and painted
    /// as the cut line so the editor sees the frame it is about to land on
    /// before pressing.
    razor_time: Option<RationalTime>,
    /// A split asked for from outside a painted frame — the `Ctrl+K` shortcut
    /// — waiting for the next frame, which has the sequence to plan against.
    pending_split: bool,
    /// The lane the keyboard edits land on: the last one the editor pointed
    /// at.
    ///
    /// Comma and period edit from the bin without a pointer, so they need a
    /// destination of their own; every other editor calls it the target track
    /// and moves it with the last click, which is what this follows.
    target_track: usize,
    /// What the item being dragged out of the bin would do if it were dropped
    /// where the pointer is, recomputed each frame.
    ///
    /// `Ok` is the edit that will be committed on release and the span it
    /// occupies; `Err` is why it will not be, which is what paints the refusal
    /// and its hint before the button comes up.
    drop_plan: Option<Result<PlannedEdit, SourceRefusal>>,
}

/// A gesture that started in the lanes and is still under the pointer.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Gesture {
    /// A rubber band selecting everything it covers.
    Marquee {
        /// Where the band was pressed.
        origin: Pos2,
        /// Where the pointer is now.
        current: Pos2,
    },
    /// A razor press that has already cut, still under the button.
    ///
    /// The razor has no drag: the cut happens on the press, and this is what
    /// keeps the frames the button stays down from cutting again.
    Cut,
    /// The selection being dragged to a new time, and maybe a new track.
    Move {
        /// Where the drag was pressed.
        origin: Pos2,
        /// Where the pointer is now.
        current: Pos2,
    },
    /// One audio clip's fade handle being dragged.
    Fade {
        /// Where the drag was pressed.
        origin: Pos2,
        /// Where the pointer is now.
        current: Pos2,
        /// The clip whose handle is held.
        target: ClipRef,
        /// The handle that is held.
        edge: FadeEdge,
    },
    /// One clip's edge being dragged to a new time.
    Trim {
        /// Where the drag was pressed.
        origin: Pos2,
        /// Where the pointer is now.
        current: Pos2,
        /// The clip whose edge is held.
        target: ClipRef,
        /// The edge that is held.
        edge: TrimEdge,
    },
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
    /// The label of the undo group the header gesture that began this frame
    /// opens, when one began.
    ///
    /// A gain drag raises one action a frame so the mixer follows the pointer,
    /// and they all belong to one entry in the undo stack; this and
    /// [`TimelineResponse::actions_commit`] are that entry's brackets
    /// ([`apply_actions`](crate::track_header::apply_actions) applies them).
    pub actions_begin: Option<String>,
    /// Whether the header gesture ended this frame, closing the group.
    pub actions_commit: bool,
    /// Where the ruler was clicked or scrubbed to this frame, snapped.
    ///
    /// The panel has already moved its own playhead there so the frame it
    /// paints is the frame the user asked for; the caller applies the same
    /// time to the viewer and the playback scheduler, which own playback.
    pub seek: Option<RationalTime>,
    /// What [`TimelineResponse::seek`] snapped to, when it snapped.
    pub snapped: Option<SnapCandidate>,
    /// Whether the selection changed this frame.
    pub selection_changed: bool,
    /// The move a released clip drag asks for, as one undoable group.
    ///
    /// The panel has moved nothing: it plans the drag and hands the plan over,
    /// for the caller to apply with
    /// [`apply_move`](crate::selection::apply_move) so the whole drag is one
    /// entry in the undo stack.
    pub clip_move: Option<MoveGroup>,
    /// Why the drag under the pointer cannot become an edit, while it cannot.
    pub refused: Option<MoveRefusal>,
    /// The trim a released edge drag asks for, as one undoable group.
    ///
    /// Like [`TimelineResponse::clip_move`], the panel has trimmed nothing: it
    /// plans the drag and hands the plan over, for the caller to apply with
    /// [`apply_trim`](crate::trim::apply_trim) so the trim and every clip a
    /// ripple carried are one entry in the undo stack.
    pub clip_trim: Option<TrimGroup>,
    /// Why the trim under the pointer cannot become an edit, while it cannot.
    pub trim_refused: Option<TrimRefusal>,
    /// The fade a released handle drag asks for, as one undoable command.
    ///
    /// Like the move and the trim, the panel has faded nothing: it plans the
    /// drag and hands the plan over, for the caller to apply with
    /// [`apply_fade`](crate::fade::apply_fade).
    pub clip_fade: Option<FadeEdit>,
    /// Why the fade under the pointer cannot become an edit, while it cannot.
    pub fade_refused: Option<FadeRefusal>,
    /// The cut a razor click or `Ctrl+K` asks for, as one undoable group.
    ///
    /// The panel has cut nothing: it plans the cut and hands the plan over,
    /// for the caller to apply with [`apply_split`](crate::split::apply_split)
    /// so a through-edit across several tracks is one entry in the undo
    /// stack.
    pub clip_split: Option<SplitGroup>,
    /// Why the cut asked for this frame could not be planned.
    pub split_refused: Option<SplitRefusal>,
    /// The marker actions raised this frame, in the order they were raised.
    ///
    /// Every one of them is exactly one command
    /// ([`MarkerAction::into_command`]), so dropping, dragging, renaming and
    /// deleting a marker are all undoable.
    pub marker_actions: Vec<MarkerAction>,
    /// The edit an item dropped from the bin asks for, planned and ready to
    /// apply with
    /// [`apply_source_edit`](crate::source_edit::apply_source_edit).
    ///
    /// The panel has placed nothing: a drop from the bin is one command like
    /// any other edit, and the caller is the one that owns the history.
    pub source_edit: Option<PlannedEdit>,
    /// Why the item hovering over the lanes, or the one just dropped, cannot
    /// be edited onto the track under it.
    ///
    /// Present while the pointer holds a drag the track will refuse, so the
    /// hint can be shown before the button comes up.
    pub drop_refused: Option<SourceRefusal>,
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
            waveforms: WaveformCache::new(),
            last_layout: None,
            thumbnails: ThumbnailCache::default(),
            playhead: RationalTime::zero(rate),
            snap: SnapSettings::default(),
            scrubbing: false,
            candidates: Vec::new(),
            snapped: None,
            selection: Selection::new(),
            gesture: None,
            marquee_base: Vec::new(),
            drag_plan: None,
            trim_plan: None,
            hovered_trim: None,
            fade_plan: None,
            hovered_fade: None,
            marker_state: MarkerState::new(),
            marker_press: false,
            marker_grab_offset: 0.0,
            pending_markers: Vec::new(),
            tool: Tool::Select,
            razor_time: None,
            pending_split: false,
            target_track: 0,
            drop_plan: None,
        }
    }

    /// Which tool a press in the lanes drives.
    #[must_use]
    pub const fn tool(&self) -> Tool {
        self.tool
    }

    /// Picks the tool a press in the lanes drives.
    ///
    /// Putting the razor away drops the cut line with it, so the panel does
    /// not keep painting a cut that can no longer be made.
    pub const fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        if !matches!(tool, Tool::Razor) {
            self.razor_time = None;
        }
    }

    /// Where the razor would cut if it were clicked now, while it is out.
    #[must_use]
    pub const fn razor_time(&self) -> Option<RationalTime> {
        self.razor_time
    }

    /// Asks for a cut at the playhead on the next painted frame.
    ///
    /// This is what [`Action::SplitAtPlayhead`](crate::shortcuts::Action) —
    /// `Ctrl+K` — calls. The cut is not planned here because planning it
    /// takes the sequence, which the panel only sees while it paints; the
    /// plan reaches the caller as [`TimelineResponse::clip_split`] on the
    /// next frame, exactly as a razor click's does.
    pub const fn request_split_at_playhead(&mut self) {
        self.pending_split = true;
    }

    /// The clips the editor has selected.
    #[must_use]
    pub const fn selection(&self) -> &Selection {
        &self.selection
    }

    /// The selection, mutably, for the commands that select from outside the
    /// lanes: select all, select none, a clip picked in the inspector.
    pub const fn selection_mut(&mut self) -> &mut Selection {
        &mut self.selection
    }

    /// What the drag under the pointer would commit, while one is in progress
    /// and legal.
    #[must_use]
    pub fn drag_preview(&self) -> Option<&MoveGroup> {
        self.drag_plan.as_ref().and_then(|plan| plan.as_ref().ok())
    }

    /// The lane a keyboard edit from the bin lands on.
    ///
    /// The last lane the editor pointed at, and the first one until they point
    /// at any.
    #[must_use]
    pub const fn target_track(&self) -> usize {
        self.target_track
    }

    /// Aims the keyboard edits at lane `index`.
    pub const fn set_target_track(&mut self, index: usize) {
        self.target_track = index;
    }

    /// What dropping the item now held over the lanes would do, while it would
    /// do anything.
    #[must_use]
    pub fn drop_preview(&self) -> Option<&PlannedEdit> {
        self.drop_plan.as_ref().and_then(|plan| plan.as_ref().ok())
    }

    /// Why the item now held over the lanes cannot be dropped there, while it
    /// cannot.
    #[must_use]
    pub fn drop_refusal(&self) -> Option<SourceRefusal> {
        self.drop_plan
            .as_ref()
            .and_then(|plan| plan.as_ref().err().copied())
    }

    /// Plans an edit of `media` from the bin at the playhead, on the target
    /// track.
    ///
    /// This is what comma and period ask for: the same plan a drop makes, with
    /// the playhead for the instant and [`TimelinePanel::target_track`] for the
    /// destination. Nothing is applied; the caller puts the plan through the
    /// Command API.
    ///
    /// # Errors
    ///
    /// The [`SourceRefusal`] saying why the edit cannot be made.
    pub fn plan_edit_at_playhead(
        &self,
        project: &Project,
        sequence: &Sequence,
        media: sub_model::MediaId,
        mode: EditMode,
    ) -> Result<PlannedEdit, SourceRefusal> {
        plan_source_edit(
            project,
            sequence,
            media,
            self.target_track,
            self.playhead,
            mode,
        )
    }

    /// Why the drag under the pointer would be refused, while one is.
    #[must_use]
    pub fn drag_refusal(&self) -> Option<MoveRefusal> {
        match self.drag_plan {
            Some(Err(refusal)) => Some(refusal),
            _ => None,
        }
    }

    /// Whether a clip drag is under the pointer.
    #[must_use]
    pub const fn is_dragging_clips(&self) -> bool {
        matches!(self.gesture, Some(Gesture::Move { .. }))
    }

    /// What the trim under the pointer would commit, while one is in progress
    /// and legal.
    #[must_use]
    pub fn trim_preview(&self) -> Option<&TrimGroup> {
        self.trim_plan.as_ref().and_then(|plan| plan.as_ref().ok())
    }

    /// Why the trim under the pointer would be refused, while one is.
    #[must_use]
    pub const fn trim_refusal(&self) -> Option<TrimRefusal> {
        match self.trim_plan {
            Some(Err(refusal)) => Some(refusal),
            _ => None,
        }
    }

    /// What the fade under the pointer would commit, while a drag is in
    /// progress and legal.
    #[must_use]
    pub fn fade_preview(&self) -> Option<&FadeEdit> {
        self.fade_plan.as_ref().and_then(|plan| plan.as_ref().ok())
    }

    /// Why the fade under the pointer would be refused, while one is.
    #[must_use]
    pub const fn fade_refusal(&self) -> Option<FadeRefusal> {
        match self.fade_plan {
            Some(Err(refusal)) => Some(refusal),
            _ => None,
        }
    }

    /// Whether a fade drag is under the pointer.
    #[must_use]
    pub const fn is_fading(&self) -> bool {
        matches!(self.gesture, Some(Gesture::Fade { .. }))
    }

    /// The fade handle the pointer is over.
    #[must_use]
    pub const fn hovered_fade(&self) -> Option<(ClipRef, FadeEdge)> {
        self.hovered_fade
    }

    /// Whether a trim drag is under the pointer.
    #[must_use]
    pub const fn is_trimming(&self) -> bool {
        matches!(self.gesture, Some(Gesture::Trim { .. }))
    }

    /// The clip edge the pointer is over, which shows the trim cursor.
    #[must_use]
    pub const fn hovered_trim(&self) -> Option<(ClipRef, TrimEdge)> {
        self.hovered_trim
    }

    /// The rubber band under the pointer, while a marquee is in progress.
    #[must_use]
    pub fn marquee_rect(&self) -> Option<Rect> {
        match self.gesture {
            Some(Gesture::Marquee { origin, current }) => Some(Rect::from_two_pos(origin, current)),
            _ => None,
        }
    }

    /// The ruler's marker state: what is selected, dragged or being renamed.
    #[must_use]
    pub const fn marker_state(&self) -> &MarkerState {
        &self.marker_state
    }

    /// The marker state, mutably, for the selection to be set from elsewhere.
    pub const fn marker_state_mut(&mut self) -> &mut MarkerState {
        &mut self.marker_state
    }

    /// Asks for a marker at the playhead, as the `M` shortcut does.
    ///
    /// The marker is minted here and raised on the next painted frame as a
    /// [`MarkerAction::Add`] in [`TimelineResponse::marker_actions`], so a
    /// shortcut and a gesture reach the Command API by the same road. The new
    /// identifier is returned so the caller can follow it.
    ///
    /// The playhead is view state, so nothing is added to the project until
    /// the caller applies the command.
    pub fn add_marker_at_playhead(&mut self) -> MarkerId {
        let action = MarkerAction::add_at(self.playhead, DEFAULT_MARKER_NAME);
        let id = action.marker();
        self.pending_markers.push(action);
        id
    }

    /// Where the flag of a marker starting at `time` sits on the ruler.
    ///
    /// `None` before the first painted frame, when the panel does not yet
    /// know where it is.
    #[must_use]
    pub fn marker_flag_rect(&self, time: RationalTime) -> Option<Rect> {
        let layout = self.last_layout?;
        let x = layout.content.left() + self.view.pixel_of(time);
        Some(Rect::from_min_size(
            pos2(x, layout.ruler.bottom() - MARKER_FLAG_HEIGHT),
            Vec2::new(MARKER_FLAG_WIDTH, MARKER_FLAG_HEIGHT),
        ))
    }

    /// The marker whose flag `pos` lands on, if any.
    ///
    /// Later markers win: they are painted over earlier ones, so what the eye
    /// says is on top is what the pointer grabs.
    #[must_use]
    pub fn marker_at(&self, sequence: &Sequence, pos: eframe::egui::Pos2) -> Option<MarkerId> {
        sequence.markers.iter().rev().find_map(|marker| {
            let rect = self
                .marker_flag_rect(self.shown_start(marker))?
                .expand2(Vec2::new(MARKER_GRAB_SLACK, 0.0));
            rect.contains(pos).then_some(marker.id)
        })
    }

    /// Where a marker's head is drawn, which during a drag is where the
    /// pointer has taken it rather than where the project still has it.
    fn shown_start(&self, marker: &Marker) -> RationalTime {
        if self.marker_state.dragging() == Some(marker.id) {
            self.marker_state
                .drag_time()
                .unwrap_or_else(|| marker.marked_range.start())
        } else {
            marker.marked_range.start()
        }
    }

    /// Where the playhead is, in sequence time.
    #[must_use]
    pub const fn playhead(&self) -> RationalTime {
        self.playhead
    }

    /// Puts the playhead at `time`, clamped at the start of the sequence.
    ///
    /// This is how the application keeps the panel in step with the viewer
    /// and the playback clock: the playhead is view state, so it is set, not
    /// commanded.
    pub fn set_playhead(&mut self, time: RationalTime) {
        let rate = self.view.rate();
        let time = time.rescaled_to(rate);
        self.playhead = if time.is_negative() {
            RationalTime::zero(rate)
        } else {
            time
        };
    }

    /// Whether times dropped on the timeline snap, and how close counts.
    #[must_use]
    pub const fn snap_settings(&self) -> SnapSettings {
        self.snap
    }

    /// The snap settings, mutably, to change the threshold or set the flag
    /// from a restored workspace.
    pub const fn snap_settings_mut(&mut self) -> &mut SnapSettings {
        &mut self.snap
    }

    /// Turns snapping on or off, and reports the new state.
    ///
    /// This is what [`Action::ToggleSnapping`](crate::shortcuts::Action) —
    /// `S` by default — is wired to.
    pub const fn toggle_snapping(&mut self) -> bool {
        self.snap.toggle()
    }

    /// The snap targets gathered by the last call to
    /// [`TimelinePanel::collect_snap_candidates`].
    #[must_use]
    pub fn snap_candidates(&self) -> &[SnapCandidate] {
        &self.candidates
    }

    /// Gathers the snap targets inside the viewport into the panel's buffer.
    ///
    /// `include_playhead` is what separates the two users of snapping: a clip
    /// being dragged snaps to the playhead, but the playhead cannot snap to
    /// itself, so a ruler scrub leaves it out. Everything else — clip edges,
    /// markers, the head of the sequence — is the same for both.
    ///
    /// Only the clips the viewport touches are considered, through the same
    /// indexes the painter uses, so this costs a binary search per track
    /// rather than a walk of the sequence.
    pub fn collect_snap_candidates(&mut self, sequence: &Sequence, include_playhead: bool) {
        let playhead = include_playhead.then_some(self.playhead);
        let mut candidates = std::mem::take(&mut self.candidates);
        snapping::collect_candidates(
            sequence,
            &self.layouts,
            playhead,
            self.view.visible_range(),
            &mut candidates,
        );
        self.candidates = candidates;
    }

    /// `time` pulled onto the nearest snap target within the threshold.
    ///
    /// Call [`TimelinePanel::collect_snap_candidates`] first. Returns the
    /// candidate it landed on, or `None` when snapping is off or nothing is
    /// close enough.
    #[must_use]
    pub fn snap(&self, time: RationalTime) -> Option<SnapCandidate> {
        snapping::snap(&self.view, time, &self.candidates, self.snap)
    }

    /// The thumbnail textures the clip strips are painted from.
    #[must_use]
    pub const fn thumbnails(&self) -> &ThumbnailCache {
        &self.thumbnails
    }

    /// The thumbnail cache, mutably.
    ///
    /// This is where generated strips are handed in
    /// ([`ThumbnailCache::insert_strip`]) and where the media the timeline
    /// wants a strip for is read back
    /// ([`ThumbnailCache::take_missing`]); the panel itself never queues a
    /// thumbnail job.
    pub const fn thumbnails_mut(&mut self) -> &mut ThumbnailCache {
        &mut self.thumbnails
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

    /// The waveform cache the audio clips are drawn from.
    ///
    /// The panel neither generates nor reads peaks: the application hands them
    /// over with [`WaveformCache::insert`] as the waveform jobs finish, and the
    /// panel turns whatever is there into textures the first time it draws the
    /// media.
    #[must_use]
    pub const fn waveforms(&self) -> &WaveformCache {
        &self.waveforms
    }

    /// The waveform cache, to hand it peaks or to forget them.
    #[must_use]
    pub const fn waveforms_mut(&mut self) -> &mut WaveformCache {
        &mut self.waveforms
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

    /// The header column's state: the rename in progress and the per-track
    /// level meters.
    #[must_use]
    pub fn header_state(&self) -> &TrackHeaderState {
        &self.header_state
    }

    /// The header column's state, for feeding the track meters from the
    /// mixer's meter bank once a frame.
    pub fn header_state_mut(&mut self) -> &mut TrackHeaderState {
        &mut self.header_state
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
            // The playhead is an instant, not a frame number: it follows the
            // sequence to its new timebase rather than staying at the same
            // count of a different frame.
            self.playhead = self.playhead.rescaled_to(rate);
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
        // An edit can take a selected clip away — a remove, a ripple, an undo
        // of the drag that made it — and a phantom in the selection would
        // otherwise be dragged again and refused as unknown.
        self.selection.retain_existing(sequence);
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
        let dropped = self.handle_bin_drag(&response, &layout, project, sequence);
        let mut marker_actions = self.handle_markers(ui, &response, &layout, sequence);
        let seek = self.handle_scrub(&response, &layout, sequence);
        let (shift, alt) = ui.input(|input| (input.modifiers.shift, input.modifiers.alt));
        let mut lanes = if seek.is_some() {
            LaneOutcome::default()
        } else if matches!(self.tool, Tool::Razor) {
            self.handle_razor(&response, &layout, sequence)
        } else {
            self.razor_time = None;
            self.handle_lanes(&response, &layout, project, sequence, shift, alt)
        };
        self.update_trim_hover(ui.ctx(), &response, &layout, sequence);
        // A cut asked for by the keyboard beats one asked for by the razor in
        // the same frame, which cannot happen unless a script drives both.
        if self.pending_split {
            self.pending_split = false;
            match plan_split(sequence, &self.selection, self.playhead) {
                Ok(Some(group)) => lanes.clip_split = Some(group),
                Ok(None) => {}
                Err(refusal) => lanes.split_refused = Some(refusal),
            }
        }
        self.prepare_waveforms(ui.ctx(), project, sequence, layout);
        let visuals = ui.visuals().clone();
        let painter = ui.painter().with_clip_rect(rect);
        paint_frame(&painter, &layout, &visuals);
        self.paint_ruler(&painter, &layout, &visuals);
        // The thumbnail cache is lifted out for the duration of the paint so
        // the clip loop can read the panel's indexes and fill the cache in the
        // same pass; it goes straight back afterwards.
        let mut thumbnails = std::mem::take(&mut self.thumbnails);
        thumbnails.begin_frame();
        let mut strips = StripPainter {
            ctx: ui.ctx().clone(),
            pixels_per_point: ui.pixels_per_point(),
            cache: &mut thumbnails,
        };
        self.paint_lanes(&painter, &layout, &visuals, project, sequence, &mut strips);
        self.thumbnails = thumbnails;
        self.paint_gesture(&painter, &layout, sequence);
        self.paint_razor(&painter, &layout);
        self.paint_drop_target(&painter, &layout);
        self.paint_markers(&painter, &layout, sequence);
        self.paint_playhead(&painter, &layout);
        let headers = self.header_controls(ui, &layout, sequence);
        marker_actions.extend(self.marker_rename_editor(ui, sequence));
        TimelineResponse {
            response,
            actions: headers.actions,
            actions_begin: headers.begin,
            actions_commit: headers.commit,
            seek,
            snapped: if seek.is_some() { self.snapped } else { None },
            selection_changed: lanes.selection_changed,
            clip_move: lanes.clip_move,
            refused: lanes.refused,
            clip_trim: lanes.clip_trim,
            trim_refused: lanes.trim_refused,
            clip_fade: lanes.clip_fade,
            fade_refused: lanes.fade_refused,
            clip_split: lanes.clip_split,
            split_refused: lanes.split_refused,
            marker_actions,
            source_edit: dropped.edit,
            drop_refused: dropped.refused.or_else(|| self.drop_refusal()),
        }
    }

    /// Tracks an item dragged out of the media bin, and takes the drop.
    ///
    /// While the pointer holds a bin drag over the lanes the edit it would
    /// make is planned every frame, so the target span — or the refusal and
    /// its hint — is on screen before the button comes up. The plan is made
    /// again on release rather than reused, because the pointer may have moved
    /// between the last painted frame and the release.
    ///
    /// A drop is always an overwrite: it lands where it was aimed, and nothing
    /// else moves. Comma and period are how an editor asks for the rippling
    /// kind ([`TimelinePanel::plan_edit_at_playhead`]).
    fn handle_bin_drag(
        &mut self,
        response: &Response,
        layout: &PanelLayout,
        project: &Project,
        sequence: &Sequence,
    ) -> DropOutcome {
        self.drop_plan = None;
        let mut outcome = DropOutcome::default();
        let hovering = response.dnd_hover_payload::<BinDrag>();
        let released = response.dnd_release_payload::<BinDrag>();
        let Some(drag) = released.as_deref().or(hovering.as_deref()) else {
            return outcome;
        };
        let Some(pos) = response
            .ctx
            .pointer_interact_pos()
            .filter(|pos| layout.content.contains(*pos))
        else {
            return outcome;
        };
        let index = self.lane_at(layout.content.top(), pos.y);
        let plan = usize::try_from(index)
            .map_err(|_| SourceRefusal::NoSuchTrack)
            .and_then(|index| {
                let time = self
                    .view
                    .time_at_pixel(round_px(pos.x - layout.content.left()));
                plan_source_edit(
                    project,
                    sequence,
                    drag.media,
                    index,
                    time,
                    EditMode::Overwrite,
                )
            });
        if released.is_some() {
            match plan {
                Ok(edit) => {
                    self.target_track = edit.track_index;
                    outcome.edit = Some(edit);
                }
                Err(refusal) => outcome.refused = Some(refusal),
            }
        } else {
            self.drop_plan = Some(plan);
        }
        outcome
    }

    /// Paints where the item held over the lanes would land: a ghost of the
    /// clip, or the refused wash where it cannot go.
    fn paint_drop_target(&self, painter: &Painter, layout: &PanelLayout) {
        let lanes = painter.with_clip_rect(layout.content);
        match &self.drop_plan {
            Some(Ok(edit)) => {
                let rect = self.clip_rect(layout, edit.track_index, edit.range);
                lanes.rect_filled(
                    rect,
                    CornerRadius::same(2),
                    tint(SELECTION_COLOR, GHOST_ALPHA),
                );
                lanes.rect_stroke(
                    rect,
                    CornerRadius::same(2),
                    Stroke::new(SELECTION_WIDTH, SELECTION_COLOR),
                    StrokeKind::Inside,
                );
            }
            Some(Err(_)) => {
                let Some(index) = usize::try_from(
                    self.lane_at(
                        layout.content.top(),
                        painter
                            .ctx()
                            .pointer_interact_pos()
                            .map_or(layout.content.top(), |pos| pos.y),
                    ),
                )
                .ok() else {
                    return;
                };
                let top = self.lane_top(layout.content.top(), index);
                let lane = Rect::from_min_size(
                    pos2(layout.content.left(), top),
                    Vec2::new(layout.content.width(), self.metrics.track_height),
                );
                lanes.rect_filled(lane, CornerRadius::ZERO, tint(REFUSED_COLOR, REFUSED_ALPHA));
            }
            None => {}
        }
    }

    /// Turns a press, a drag, a double-click or a `Delete` on a marker into
    /// the command it asks for.
    ///
    /// This runs before [`TimelinePanel::handle_scrub`] and claims the press
    /// when it lands on a flag, so grabbing a marker drags it rather than
    /// scrubbing the ruler out from under it. A press anywhere else on the
    /// ruler is left to the scrub, and clears the selection.
    fn handle_markers(
        &mut self,
        ui: &Ui,
        response: &Response,
        layout: &PanelLayout,
        sequence: &Sequence,
    ) -> Vec<MarkerAction> {
        // An undo, or another agent over the Command API, can take a marker
        // away between frames; the selection that named it goes with it.
        self.marker_state
            .retain(|marker| sequence.marker(marker).is_some());
        let mut actions = std::mem::take(&mut self.pending_markers);
        let held = response.is_pointer_button_down_on();
        let pointer = response.interact_pointer_pos();

        // A double-click on a flag opens the rename editor. It arrives on the
        // release, after the press has already begun and ended a drag that
        // went nowhere, which raises no command.
        if response.double_clicked()
            && let Some(pos) = pointer
            && let Some(marker) = self.marker_at(sequence, pos)
            && let Some(found) = sequence.marker(marker)
        {
            self.marker_state.cancel_drag();
            self.marker_state.begin_rename(marker, &found.name);
            self.marker_press = false;
            return actions;
        }

        if self.marker_state.dragging().is_some() {
            if held && let Some(pos) = pointer {
                let time = self.dragged_time(layout, pos.x, sequence);
                self.marker_state.drag_to(time);
            }
            if !held {
                actions.extend(self.marker_state.end_drag(sequence));
                self.marker_press = false;
            }
            return actions;
        }

        if !held {
            self.marker_press = false;
        } else if !self.marker_press
            && !self.scrubbing
            && let Some(pos) = pointer
            && let Some(marker) = self.marker_at(sequence, pos)
            && let Some(found) = sequence.marker(marker)
        {
            self.marker_press = true;
            let anchor = found.marked_range.start();
            self.marker_grab_offset = pos.x - layout.content.left() - self.view.pixel_of(anchor);
            self.marker_state.begin_drag(marker, anchor);
        } else if !self.marker_press && !self.scrubbing && pointer.is_some() {
            // A press on the ruler that missed every flag is a scrub, and
            // deselects.
            self.marker_state.select(None);
        }

        // Delete removes the selected marker, unless a name is being typed
        // into, where Delete belongs to the text editor.
        if self.marker_state.renaming().is_none()
            && ui.input(|input| input.key_pressed(Key::Delete) || input.key_pressed(Key::Backspace))
        {
            actions.extend(self.marker_state.remove_selected());
        }
        actions
    }

    /// The instant the marker being dragged has been taken to.
    ///
    /// Snapped like every other time dropped on the timeline, so a marker
    /// lands cleanly on a cut or on the playhead. The marker's own ends are
    /// left out of the candidates: a marker cannot snap to where it already
    /// is.
    fn dragged_time(&mut self, layout: &PanelLayout, x: f32, sequence: &Sequence) -> RationalTime {
        let rate = self.view.rate();
        let head_px = x - layout.content.left() - self.marker_grab_offset;
        let raw = self.view.time_at_pixel(round_px(head_px));
        let raw = if raw.is_negative() {
            RationalTime::zero(rate)
        } else {
            raw
        };
        self.collect_snap_candidates(sequence, true);
        if let Some(dragged) = self
            .marker_state
            .dragging()
            .and_then(|marker| sequence.marker(marker))
        {
            let start = dragged.marked_range.start();
            let end = dragged.marked_range.end_exclusive();
            self.candidates.retain(|candidate| {
                candidate.kind != SnapKind::Marker
                    || (candidate.time != start && candidate.time != end)
            });
        }
        self.snap(raw).map_or(raw, |candidate| candidate.time)
    }

    /// Runs the inline rename editor over the marker being renamed, if one is.
    ///
    /// The editor commits on Enter and on losing focus and abandons the edit
    /// on Escape, exactly as the track header's does, and opens with the old
    /// name selected so the first keystroke replaces it.
    fn marker_rename_editor(&mut self, ui: &mut Ui, sequence: &Sequence) -> Option<MarkerAction> {
        let id = self.marker_state.renaming()?;
        let marker = sequence.marker(id)?;
        let rect = self
            .marker_flag_rect(marker.marked_range.start())?
            .translate(Vec2::new(MARKER_FLAG_WIDTH + MARKER_NAME_GAP, 0.0));
        let rect =
            Rect::from_min_size(rect.min, Vec2::new(MARKER_RENAME_WIDTH, MARKER_FLAG_HEIGHT));
        let widget_id = ui.id().with(("marker_rename", id));
        let mut text = self
            .marker_state
            .rename_text()
            .unwrap_or_default()
            .to_owned();
        let focused = ui.memory(|memory| memory.has_focus(widget_id));
        if self.marker_state.rename_is_fresh() {
            crate::track_header::select_all(ui, widget_id, &text);
            if focused {
                self.marker_state.rename_focused();
            }
        }
        let mut editor = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("marker_rename"));
        let response = editor.put(
            rect,
            TextEdit::singleline(&mut text)
                .id(widget_id)
                .desired_width(rect.width())
                .font(TextStyle::Small),
        );
        if let Some(buffer) = self.marker_state.rename_buffer() {
            *buffer = text;
        }
        if !response.has_focus() && !response.lost_focus() {
            response.request_focus();
        }
        if ui.input(|input| input.key_pressed(Key::Escape)) {
            self.marker_state.cancel_rename();
            return None;
        }
        if response.lost_focus() || ui.input(|input| input.key_pressed(Key::Enter)) {
            return self.marker_state.commit_rename(&marker.name);
        }
        None
    }

    /// Paints the sequence's markers: a coloured flag on the ruler with the
    /// name beside it, and a faint guide line of the same colour down the
    /// lanes.
    ///
    /// The marker being dragged is painted where the pointer has taken it
    /// rather than where the project still has it, so the drag reads as the
    /// move it is about to become.
    fn paint_markers(&self, painter: &Painter, layout: &PanelLayout, sequence: &Sequence) {
        if sequence.markers.is_empty() {
            return;
        }
        let ruler = painter.with_clip_rect(layout.ruler);
        let lanes = painter.with_clip_rect(layout.content);
        let selected = self.marker_state.selected();
        let renaming = self.marker_state.renaming();
        for marker in &sequence.markers {
            let start = self.shown_start(marker);
            let x = layout.content.left() + self.view.pixel_of(start);
            if x < layout.content.left() - MARKER_FLAG_WIDTH || x > layout.content.right() + 1.0 {
                continue;
            }
            let color = markers::marker_color(marker.id);
            lanes.line_segment(
                [
                    pos2(x, layout.content.top()),
                    pos2(x, layout.content.bottom()),
                ],
                Stroke::new(
                    1.0,
                    color.gamma_multiply(f32::from(MARKER_GUIDE_ALPHA) / 255.0),
                ),
            );
            // A span marker gets a bar across what it covers, so a note over a
            // passage reads as a passage rather than as a point.
            if !marker.marked_range.is_empty() {
                let end = layout.content.left()
                    + self.view.pixel_of(start + marker.marked_range.duration());
                ruler.rect_filled(
                    Rect::from_min_max(
                        pos2(x, layout.ruler.bottom() - MARKER_SPAN_HEIGHT),
                        pos2(end, layout.ruler.bottom()),
                    ),
                    CornerRadius::ZERO,
                    color,
                );
            }
            let flag = Rect::from_min_size(
                pos2(x, layout.ruler.bottom() - MARKER_FLAG_HEIGHT),
                Vec2::new(MARKER_FLAG_WIDTH, MARKER_FLAG_HEIGHT),
            );
            ruler.rect_filled(flag, CornerRadius::same(2), color);
            ruler.line_segment(
                [pos2(x, layout.ruler.top()), pos2(x, layout.ruler.bottom())],
                Stroke::new(1.0, color),
            );
            if selected == Some(marker.id) {
                ruler.rect_stroke(
                    flag.expand(1.0),
                    CornerRadius::same(2),
                    Stroke::new(MARKER_SELECTED_WIDTH, Color32::WHITE),
                    StrokeKind::Outside,
                );
            }
            // The name is the editor's while it is being renamed.
            if renaming == Some(marker.id) || marker.name.is_empty() {
                continue;
            }
            // Over a plate, because a marker can sit under a timecode label
            // and the name has to stay readable when it does.
            let galley = ruler.layout_no_wrap(
                marker.name.clone(),
                FontId::proportional(10.0),
                MARKER_NAME_COLOR,
            );
            let top_left = pos2(
                flag.right() + MARKER_NAME_GAP,
                flag.center().y - galley.size().y / 2.0,
            );
            ruler.rect_filled(
                Rect::from_min_size(top_left, galley.size()).expand(1.0),
                CornerRadius::same(2),
                Color32::from_black_alpha(NAME_PLATE_ALPHA),
            );
            ruler.galley(top_left, galley, MARKER_NAME_COLOR);
        }
    }

    /// Turns a press or a drag in the ruler into a seek.
    ///
    /// A press in the ruler starts a scrub and every frame the button stays
    /// down continues it, so a click and a drag are the same gesture: the
    /// difference is only how long it lasts. The instant under the pointer is
    /// snapped before it is reported, and the panel moves its own playhead
    /// straight away so the frame it is about to paint already shows the
    /// answer.
    ///
    /// A press anywhere else — a lane, the header column — is not a seek and
    /// leaves the playhead alone: it belongs to clip selection instead, which
    /// [`TimelinePanel::handle_lanes`] takes. A drag that started in a lane
    /// and wandered up into the ruler stays that drag, so a gesture in
    /// progress is never turned into a scrub half way through.
    fn handle_scrub(
        &mut self,
        response: &Response,
        layout: &PanelLayout,
        sequence: &Sequence,
    ) -> Option<RationalTime> {
        if self.gesture.is_some() {
            return None;
        }
        // A press that grabbed a marker's flag belongs to that marker for as
        // long as it lasts; the ruler under it does not scrub.
        if self.marker_press || self.marker_state.dragging().is_some() {
            self.scrubbing = false;
            return None;
        }
        let held = response.is_pointer_button_down_on();
        let Some(pos) = response.interact_pointer_pos() else {
            self.scrubbing = false;
            return None;
        };
        if !self.scrubbing {
            if !layout.ruler.contains(pos) {
                self.scrubbing = false;
                return None;
            }
            self.scrubbing = true;
        }
        if !held {
            self.scrubbing = false;
        }
        let rate = self.view.rate();
        let raw = self
            .view
            .time_at_pixel(round_px(pos.x - layout.content.left()));
        let raw = if raw.is_negative() {
            RationalTime::zero(rate)
        } else {
            raw
        };
        // The playhead is not a snap target for itself, so it is left out of
        // the candidates a ruler scrub is resolved against.
        self.collect_snap_candidates(sequence, false);
        self.snapped = self.snap(raw);
        let time = self.snapped.map_or(raw, |candidate| candidate.time);
        self.playhead = time;
        Some(time)
    }

    /// Turns a press, drag or release in the lanes into selection and moves.
    ///
    /// The three gestures the lanes have are told apart by what is under the
    /// press: a clip starts a move of whatever is selected, empty lane starts
    /// a marquee, and shift makes either one add to the selection instead of
    /// replacing it. Everything the drag computes is exact — the offset is a
    /// difference of two [`RationalTime`]s and the track offset a difference
    /// of two lane indexes — so the preview on screen and the commands
    /// committed on release come from the same numbers.
    ///
    /// Nothing is mutated but the panel's own view state: the move is planned
    /// here and applied by the caller through the Command API.
    fn handle_lanes(
        &mut self,
        response: &Response,
        layout: &PanelLayout,
        project: &Project,
        sequence: &Sequence,
        shift: bool,
        alt: bool,
    ) -> LaneOutcome {
        let mut outcome = LaneOutcome::default();
        let held = response.is_pointer_button_down_on();
        let pointer = response.interact_pointer_pos();
        let Some(gesture) = self.gesture else {
            if let Some(pos) = pointer.filter(|_| held)
                && layout.content.contains(pos)
            {
                outcome.selection_changed = self.begin_lane_gesture(pos, layout, sequence, shift);
            }
            return outcome;
        };
        match gesture {
            // A cut left under the pointer by the razor: putting the arrow
            // back does not turn it into a drag, and the button is already
            // down, so it is dropped and the next press starts a gesture.
            Gesture::Cut => {
                if !held {
                    self.gesture = None;
                }
            }
            Gesture::Marquee { origin, current } => {
                let pos = pointer.unwrap_or(current);
                self.gesture = Some(Gesture::Marquee {
                    origin,
                    current: pos,
                });
                outcome.selection_changed = self.apply_marquee(origin, pos, layout, sequence);
                if !held {
                    self.gesture = None;
                    self.marquee_base.clear();
                }
            }
            Gesture::Fade {
                origin,
                current,
                target,
                edge,
            } => {
                let pos = pointer.unwrap_or(current);
                let grip = EdgeGrip {
                    origin,
                    pos,
                    target,
                    down: held,
                    ripple: alt,
                };
                self.drag_fade(grip, edge, layout, sequence, &mut outcome);
            }
            Gesture::Trim {
                origin,
                current,
                target,
                edge,
            } => {
                let pos = pointer.unwrap_or(current);
                let grip = EdgeGrip {
                    origin,
                    pos,
                    target,
                    down: held,
                    ripple: alt,
                };
                self.drag_trim(grip, edge, layout, project, sequence, &mut outcome);
            }
            Gesture::Move { origin, current } => {
                let pos = pointer.unwrap_or(current);
                self.gesture = Some(Gesture::Move {
                    origin,
                    current: pos,
                });
                let (delta, track_delta) = self.drag_offset(origin, pos, layout);
                match plan_move(sequence, &self.selection, delta, track_delta) {
                    Ok(group) => {
                        self.drag_plan = group.clone().map(Ok);
                        if !held {
                            outcome.clip_move = group;
                        }
                    }
                    Err(refusal) => {
                        self.drag_plan = Some(Err(refusal));
                        outcome.refused = Some(refusal);
                    }
                }
                if !held {
                    self.gesture = None;
                    self.drag_plan = None;
                }
            }
        }
        outcome
    }

    /// Carries a fade drag one frame further, and commits it when the button
    /// comes up.
    ///
    /// The plan is remade every frame rather than accumulated, so the ramp on
    /// screen and the command the release commits come from the same offset.
    fn drag_fade(
        &mut self,
        grip: EdgeGrip,
        edge: FadeEdge,
        layout: &PanelLayout,
        sequence: &Sequence,
        outcome: &mut LaneOutcome,
    ) {
        self.gesture = Some(Gesture::Fade {
            origin: grip.origin,
            current: grip.pos,
            target: grip.target,
            edge,
        });
        let delta = self.trim_offset(grip.origin, grip.pos, layout);
        match plan_fade(sequence, grip.target, edge, delta) {
            Ok(edit) => {
                self.fade_plan = edit.map(Ok);
                if !grip.down {
                    outcome.clip_fade = edit;
                }
            }
            Err(refusal) => {
                self.fade_plan = Some(Err(refusal));
                outcome.fade_refused = Some(refusal);
            }
        }
        if !grip.down {
            self.gesture = None;
            self.fade_plan = None;
        }
    }

    /// Carries a trim drag one frame further, and commits it when the button
    /// comes up.
    ///
    /// [`EdgeGrip::ripple`] is the ripple modifier: it shifts the trimmed
    /// clip's downstream neighbours by exactly what the edge moved.
    fn drag_trim(
        &mut self,
        grip: EdgeGrip,
        edge: TrimEdge,
        layout: &PanelLayout,
        project: &Project,
        sequence: &Sequence,
        outcome: &mut LaneOutcome,
    ) {
        self.gesture = Some(Gesture::Trim {
            origin: grip.origin,
            current: grip.pos,
            target: grip.target,
            edge,
        });
        let delta = self.trim_offset(grip.origin, grip.pos, layout);
        match plan_trim(project, sequence, grip.target, edge, delta, grip.ripple) {
            Ok(group) => {
                self.trim_plan = group.clone().map(Ok);
                if !grip.down {
                    outcome.clip_trim = group;
                }
            }
            Err(refusal) => {
                self.trim_plan = Some(Err(refusal));
                outcome.trim_refused = Some(refusal);
            }
        }
        if !grip.down {
            self.gesture = None;
            self.trim_plan = None;
        }
    }

    /// Turns a hover and a press in the lanes into a cut.
    ///
    /// The razor has no drag: a press is a cut, at the instant under the
    /// pointer and on the clip under it. That instant is resolved through
    /// [`crate::snapping`] exactly as a scrub's is, so a cut aimed near a
    /// clip edge, a marker or the playhead lands exactly on it rather than a
    /// frame beside it. The playhead is a candidate here — matching a cut to
    /// where the viewer is looking is the commonest reason to reach for the
    /// razor at all.
    ///
    /// A press whose snapped instant lands on a clip's own head is not a
    /// refusal and not a cut: there is already an edit there.
    fn handle_razor(
        &mut self,
        response: &Response,
        layout: &PanelLayout,
        sequence: &Sequence,
    ) -> LaneOutcome {
        let mut outcome = LaneOutcome::default();
        let held = response.is_pointer_button_down_on();
        // A press that has already cut stays a [`Gesture::Cut`] until the
        // button comes up, so the frames it is held for do not cut again.
        let cutting = held && matches!(self.gesture, Some(Gesture::Cut));
        if !cutting {
            // Any gesture the arrow left behind is over the moment the razor
            // comes out; a cut is never a drag.
            self.gesture = None;
            self.drag_plan = None;
        }
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos());
        let Some(pos) = pointer.filter(|pos| layout.content.contains(*pos)) else {
            self.razor_time = None;
            return outcome;
        };
        let at = self.razor_target(pos, layout, sequence);
        self.razor_time = Some(at);
        if !held || cutting {
            return outcome;
        }
        self.gesture = Some(Gesture::Cut);
        let Some(item) = self.clip_at(pos, layout, sequence) else {
            // Empty lane, or a locked track: nothing to cut, and nothing to
            // complain about either.
            return outcome;
        };
        match plan_split_clip(sequence, item, at) {
            Ok(group) => outcome.clip_split = Some(group),
            Err(refusal) => outcome.split_refused = Some(refusal),
        }
        outcome
    }

    /// The instant a razor at `pos` would cut at, snapped.
    fn razor_target(
        &mut self,
        pos: Pos2,
        layout: &PanelLayout,
        sequence: &Sequence,
    ) -> RationalTime {
        let rate = self.view.rate();
        let raw = self
            .view
            .time_at_pixel(round_px(pos.x - layout.content.left()));
        let raw = if raw.is_negative() {
            RationalTime::zero(rate)
        } else {
            raw
        };
        self.collect_snap_candidates(sequence, true);
        self.snap(raw).map_or(raw, |candidate| candidate.time)
    }

    /// Paints the cut line the razor is about to make.
    ///
    /// Only while the razor is out and over the lanes: the line is a promise
    /// about the next press, so with no razor and no pointer there is nothing
    /// to promise.
    fn paint_razor(&self, painter: &Painter, layout: &PanelLayout) {
        let Some(at) = self.razor_time.filter(|_| matches!(self.tool, Tool::Razor)) else {
            return;
        };
        let x = layout.content.left() + self.view.pixel_of(at);
        if !(layout.content.left()..=layout.content.right()).contains(&x) {
            return;
        }
        let lanes = painter.with_clip_rect(layout.content);
        lanes.line_segment(
            [
                pos2(x, layout.content.top()),
                pos2(x, layout.content.bottom()),
            ],
            Stroke::new(RAZOR_WIDTH, RAZOR_COLOR),
        );
        // A small blade at the head of the line, so the razor reads as a tool
        // rather than as another playhead.
        let half = RAZOR_BLADE_WIDTH / 2.0;
        let top = layout.content.top();
        lanes.add(eframe::egui::Shape::convex_polygon(
            vec![
                pos2(x - half, top),
                pos2(x + half, top),
                pos2(x, top + RAZOR_BLADE_HEIGHT),
            ],
            RAZOR_COLOR,
            Stroke::NONE,
        ));
    }

    /// Starts the gesture a press at `pos` belongs to, and says whether the
    /// selection changed.
    ///
    /// A press on an already-selected clip keeps the selection as it is, so
    /// dragging a group of clips does not collapse it to the one that happened
    /// to be under the pointer.
    fn begin_lane_gesture(
        &mut self,
        pos: Pos2,
        layout: &PanelLayout,
        sequence: &Sequence,
        shift: bool,
    ) -> bool {
        // Pointing at a lane aims the keyboard edits at it, whether or not
        // there is a clip under the pointer.
        if let Ok(index) = usize::try_from(self.lane_at(layout.content.top(), pos.y))
            && index < sequence.tracks.len()
        {
            self.target_track = index;
        }
        if let Some((target, edge)) = self.fade_target_at(pos, layout, sequence) {
            // A fade handle sits inside the clip's own edge, so it is asked
            // about first: the press takes hold of the handle rather than
            // trimming the clip under it.
            let changed = self.selection.select_only(target);
            self.gesture = Some(Gesture::Fade {
                origin: pos,
                current: pos,
                target,
                edge,
            });
            return changed;
        }
        if let Some((target, edge)) = self.trim_target_at(pos, layout, sequence) {
            // An edge is a trim, not a move: the press selects the clip so the
            // gesture is visible, and takes hold of the edge.
            let changed = self.selection.select_only(target);
            self.gesture = Some(Gesture::Trim {
                origin: pos,
                current: pos,
                target,
                edge,
            });
            return changed;
        }
        if let Some(item) = self.clip_at(pos, layout, sequence) {
            let changed = if shift {
                self.selection.toggle(item)
            } else if self.selection.contains(item) {
                false
            } else {
                self.selection.select_only(item)
            };
            self.gesture = Some(Gesture::Move {
                origin: pos,
                current: pos,
            });
            return changed;
        }
        // Empty lane, or a clip on a locked track, which cannot be edited and
        // so is not selectable: a marquee, over what shift kept.
        let changed = !shift && self.selection.clear();
        self.marquee_base = self.selection.items().to_vec();
        self.gesture = Some(Gesture::Marquee {
            origin: pos,
            current: pos,
        });
        changed
    }

    /// Selects everything the band from `origin` to `current` covers, on top
    /// of whatever the marquee started with.
    fn apply_marquee(
        &mut self,
        origin: Pos2,
        current: Pos2,
        layout: &PanelLayout,
        sequence: &Sequence,
    ) -> bool {
        let mut items = self.marquee_base.clone();
        for item in self.marquee_hits(origin, current, layout, sequence) {
            if !items.contains(&item) {
                items.push(item);
            }
        }
        self.selection.set(items)
    }

    /// The clips the band from `origin` to `current` covers.
    fn marquee_hits(
        &self,
        origin: Pos2,
        current: Pos2,
        layout: &PanelLayout,
        sequence: &Sequence,
    ) -> Vec<ClipRef> {
        let rate = self.view.rate();
        let left = layout.content.left();
        let (from_x, to_x) = ordered(origin.x, current.x);
        let start = self.view.time_at_pixel(round_px(from_x - left));
        let start = if start.is_negative() {
            RationalTime::zero(rate)
        } else {
            start
        };
        let end = self.view.time_at_pixel(round_px(to_x - left));
        let Some(span) = TimeRange::from_start_end(start, end.max(start)) else {
            return Vec::new();
        };
        let (from_y, to_y) = ordered(origin.y, current.y);
        let top = layout.content.top();
        let first = usize::try_from(self.lane_at(top, from_y).max(0)).unwrap_or(0);
        let last = usize::try_from(self.lane_at(top, to_y).max(0))
            .unwrap_or(0)
            .saturating_add(1)
            .min(sequence.tracks.len());
        clips_in_marquee(sequence, first..last.max(first), span)
    }

    /// How far a drag from `origin` to `current` moves a clip: an exact time
    /// offset, and a whole number of lanes.
    fn drag_offset(
        &self,
        origin: Pos2,
        current: Pos2,
        layout: &PanelLayout,
    ) -> (RationalTime, isize) {
        let rate = self.view.rate();
        let left = layout.content.left();
        let from = self.view.time_at_pixel(round_px(origin.x - left));
        let to = self.view.time_at_pixel(round_px(current.x - left));
        let delta = to
            .checked_sub(from)
            .unwrap_or_else(|| RationalTime::zero(rate));
        let top = layout.content.top();
        let lanes = self
            .lane_at(top, current.y)
            .saturating_sub(self.lane_at(top, origin.y));
        (delta, isize::try_from(lanes).unwrap_or(0))
    }

    /// How far a trim drag from `origin` to `current` moves an edge: an exact
    /// time offset at the sequence timebase, positive to the right.
    fn trim_offset(&self, origin: Pos2, current: Pos2, layout: &PanelLayout) -> RationalTime {
        let rate = self.view.rate();
        let left = layout.content.left();
        let from = self.view.time_at_pixel(round_px(origin.x - left));
        let to = self.view.time_at_pixel(round_px(current.x - left));
        to.checked_sub(from)
            .unwrap_or_else(|| RationalTime::zero(rate))
    }

    /// The fade handle under `pos`, when the pointer is close enough to one.
    ///
    /// Only audio lanes carry fade handles, and only the band across the top
    /// of a clip does: everything below it is still a move or a trim.
    fn fade_target_at(
        &self,
        pos: Pos2,
        layout: &PanelLayout,
        sequence: &Sequence,
    ) -> Option<(ClipRef, FadeEdge)> {
        if !layout.content.contains(pos) {
            return None;
        }
        let index = usize::try_from(self.lane_at(layout.content.top(), pos.y)).ok()?;
        let track = sequence.tracks.get(index)?;
        if !matches!(track.kind, TrackKind::Audio) || !clip_edits_allowed(track) {
            return None;
        }
        // A handle sitting on the clip's far edge is a pixel column past the
        // last frame the clip covers, so the column just inside is asked
        // about too rather than nothing being found there at all.
        let index_layout = self.layouts.get(index)?;
        let left = layout.content.left();
        let placement = [pos.x, pos.x - FADE_HANDLE_PX]
            .into_iter()
            .find_map(|x| index_layout.at(self.view.time_at_pixel(round_px(x - left))))?;
        let clip = track.clip(placement.clip)?;
        let rect = self.clip_rect(layout, index, placement.range);
        if rect.width() < MIN_FADE_WIDTH_PX || pos.y > rect.top() + FADE_BAND_PX {
            return None;
        }
        let item = ClipRef::new(track.id, placement.clip);
        let (head, tail) = self.fade_handle_x(rect, clip);
        let to_head = (pos.x - head).abs();
        let to_tail = (pos.x - tail).abs();
        if to_head <= FADE_HANDLE_PX && to_head <= to_tail {
            Some((item, FadeEdge::In))
        } else if to_tail <= FADE_HANDLE_PX {
            Some((item, FadeEdge::Out))
        } else {
            None
        }
    }

    /// Where `clip`'s two fade handles sit along its rectangle, in points.
    ///
    /// A fade of nothing puts its handle on the clip's own edge, which is what
    /// gives an editor something to grab on a clip that has never been faded.
    fn fade_handle_x(&self, rect: Rect, clip: &Clip) -> (f32, f32) {
        let rate = self.view.rate();
        let head = self.view.pixel_of(clip.fade_in.rescaled_to(rate))
            - self.view.pixel_of(RationalTime::zero(rate));
        let tail = self.view.pixel_of(clip.fade_out.rescaled_to(rate))
            - self.view.pixel_of(RationalTime::zero(rate));
        (
            (rect.left() + head).min(rect.right()),
            (rect.right() - tail).max(rect.left()),
        )
    }

    /// The clip edge under `pos`, when the pointer is close enough to one.
    ///
    /// A clip narrower than twice [`TRIM_HANDLE_PX`] splits its width between
    /// its two handles rather than letting them overlap, so the edge that is
    /// grabbed is always the nearer one.
    fn trim_target_at(
        &self,
        pos: Pos2,
        layout: &PanelLayout,
        sequence: &Sequence,
    ) -> Option<(ClipRef, TrimEdge)> {
        if !layout.content.contains(pos) {
            return None;
        }
        let index = usize::try_from(self.lane_at(layout.content.top(), pos.y)).ok()?;
        let track = sequence.tracks.get(index)?;
        if !clip_edits_allowed(track) {
            return None;
        }
        let time = self
            .view
            .time_at_pixel(round_px(pos.x - layout.content.left()));
        let placement = self.layouts.get(index)?.at(time)?;
        let rect = self.clip_rect(layout, index, placement.range);
        let grip = (rect.width() / 2.0).min(TRIM_HANDLE_PX);
        let item = ClipRef::new(track.id, placement.clip);
        if pos.x <= rect.left() + grip {
            Some((item, TrimEdge::In))
        } else if pos.x >= rect.right() - grip {
            Some((item, TrimEdge::Out))
        } else {
            None
        }
    }

    /// Notes the clip edge under the pointer and asks for the trim cursor.
    ///
    /// The cursor is the whole of the hover affordance: a handle is painted on
    /// every selected clip whatever the pointer is doing, and brightens when
    /// the pointer is on it.
    fn update_trim_hover(
        &mut self,
        ctx: &Context,
        response: &Response,
        layout: &PanelLayout,
        sequence: &Sequence,
    ) {
        // The razor never trims: while it is out, no edge is hovered and the
        // cursor stays the razor's own.
        self.hovered_trim = match self.gesture {
            _ if matches!(self.tool, Tool::Razor) => None,
            Some(Gesture::Trim { target, edge, .. }) => Some((target, edge)),
            Some(_) => None,
            None => response
                .hover_pos()
                .and_then(|pos| self.trim_target_at(pos, layout, sequence)),
        };
        self.hovered_fade = match self.gesture {
            _ if matches!(self.tool, Tool::Razor) => None,
            Some(Gesture::Fade { target, edge, .. }) => Some((target, edge)),
            Some(_) => None,
            None => response
                .hover_pos()
                .and_then(|pos| self.fade_target_at(pos, layout, sequence)),
        };
        // A fade handle beats a trim edge under the same pointer, exactly as
        // it does on a press.
        if self.hovered_fade.is_some() {
            self.hovered_trim = None;
            ctx.set_cursor_icon(CursorIcon::ResizeHorizontal);
        } else if self.hovered_trim.is_some() {
            ctx.set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
    }

    /// The clip under `pos`, when it is on an editable track.
    fn clip_at(&self, pos: Pos2, layout: &PanelLayout, sequence: &Sequence) -> Option<ClipRef> {
        if !layout.content.contains(pos) {
            return None;
        }
        let index = usize::try_from(self.lane_at(layout.content.top(), pos.y)).ok()?;
        let track = sequence.tracks.get(index)?;
        if !clip_edits_allowed(track) {
            return None;
        }
        let time = self
            .view
            .time_at_pixel(round_px(pos.x - layout.content.left()));
        let placement = self.layouts.get(index)?.at(time)?;
        Some(ClipRef::new(track.id, placement.clip))
    }

    /// The lane index the point `y` points below `top` falls in, which may be
    /// off either end of the tracks.
    fn lane_at(&self, top: f32, y: f32) -> i64 {
        let pitch = self.metrics.lane_pitch().max(1.0);
        floor_px((y - top + self.lane_scroll_px) / pitch)
    }

    /// The rectangle a clip spanning `range` on track `index` occupies.
    fn clip_rect(&self, layout: &PanelLayout, index: usize, range: TimeRange) -> Rect {
        let top = self.lane_top(layout.content.top(), index);
        let lane = Rect::from_min_size(
            pos2(layout.content.left(), top),
            Vec2::new(layout.content.width(), self.metrics.track_height),
        );
        let body = lane.shrink2(Vec2::new(0.0, 3.0));
        let left = lane.left() + self.view.pixel_of(range.start());
        let right = lane.left() + self.view.pixel_of(range.end_exclusive());
        Rect::from_min_max(
            pos2(left, body.top()),
            pos2(right.max(left + 1.0), body.bottom()),
        )
    }

    /// Paints what the gesture in progress is about to do: the rubber band of
    /// a marquee, or the ghosts of a drag.
    ///
    /// A refused drag paints no ghosts; the clips it would have moved are
    /// washed in [`REFUSED_COLOR`] by [`TimelinePanel::paint_clips`] instead,
    /// so the refusal is on the clips the editor is looking at.
    fn paint_gesture(&self, painter: &Painter, layout: &PanelLayout, sequence: &Sequence) {
        let lanes = painter.with_clip_rect(layout.content);
        if let Some(Ok(group)) = self.trim_plan.as_ref()
            && let Some(index) = sequence
                .tracks
                .iter()
                .position(|track| track.id == group.target.track)
        {
            // The ghost is the span the clip will occupy, painted over where
            // it still is, so the edge reads as being dragged to a new time.
            let rect = self.clip_rect(layout, index, group.range);
            lanes.rect_filled(
                rect,
                CornerRadius::same(3),
                tint(SELECTION_COLOR, GHOST_ALPHA),
            );
            lanes.rect_stroke(
                rect,
                CornerRadius::same(3),
                Stroke::new(1.0, SELECTION_COLOR),
                StrokeKind::Inside,
            );
        }
        match self.gesture {
            Some(Gesture::Marquee { origin, current }) => {
                let band = Rect::from_two_pos(origin, current).intersect(layout.content);
                lanes.rect_filled(
                    band,
                    CornerRadius::ZERO,
                    tint(SELECTION_COLOR, MARQUEE_ALPHA),
                );
                lanes.rect_stroke(
                    band,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, SELECTION_COLOR),
                    StrokeKind::Inside,
                );
            }
            Some(Gesture::Move { .. }) => {
                let Some(Ok(group)) = self.drag_plan.as_ref() else {
                    return;
                };
                for preview in &group.previews {
                    if preview.to_track_index >= sequence.tracks.len() {
                        continue;
                    }
                    let rect = self.clip_rect(layout, preview.to_track_index, preview.range);
                    lanes.rect_filled(
                        rect,
                        CornerRadius::same(3),
                        tint(SELECTION_COLOR, GHOST_ALPHA),
                    );
                    lanes.rect_stroke(
                        rect,
                        CornerRadius::same(3),
                        Stroke::new(SELECTION_WIDTH, SELECTION_COLOR),
                        StrokeKind::Inside,
                    );
                }
            }
            // The trimmed span's ghost is painted above, before the gesture
            // is matched on, because it is the whole of what a trim previews.
            // A fade previews itself: the clip's own ramp follows the drag.
            // The razor paints its own cut line; a press that has already cut
            // has nothing left to preview.
            Some(Gesture::Trim { .. } | Gesture::Fade { .. } | Gesture::Cut) | None => {}
        }
    }

    /// Paints the playhead: a line down the ruler and the lanes, with a head
    /// in the ruler wide enough to aim at.
    ///
    /// Nothing is painted when the playhead is scrolled off screen. The line
    /// is clipped to the ruler and the lanes so it never crosses the track
    /// header column.
    fn paint_playhead(&self, painter: &Painter, layout: &PanelLayout) {
        let x = layout.content.left() + self.view.pixel_of(self.playhead);
        if x < layout.content.left() - 1.0 || x > layout.content.right() + 1.0 {
            return;
        }
        let over = painter.with_clip_rect(Rect::from_min_max(
            pos2(layout.content.left(), layout.rect.top()),
            layout.rect.max,
        ));
        if let Some(snapped) = self.snapped.filter(|_| self.scrubbing) {
            let snap_x = layout.content.left() + self.view.pixel_of(snapped.time);
            over.line_segment(
                [
                    pos2(snap_x, layout.content.top()),
                    pos2(snap_x, layout.content.bottom()),
                ],
                Stroke::new(PLAYHEAD_WIDTH, SNAP_COLOR),
            );
        }
        over.line_segment(
            [pos2(x, layout.rect.top()), pos2(x, layout.content.bottom())],
            Stroke::new(PLAYHEAD_WIDTH, PLAYHEAD_COLOR),
        );
        let half = PLAYHEAD_HEAD_WIDTH / 2.0;
        let top = layout.ruler.top();
        over.add(eframe::egui::Shape::convex_polygon(
            vec![
                pos2(x - half, top),
                pos2(x + half, top),
                pos2(x, top + PLAYHEAD_HEAD_HEIGHT),
            ],
            PLAYHEAD_COLOR,
            Stroke::NONE,
        ));
    }

    /// Uploads the waveform textures the clips about to be painted need.
    ///
    /// Textures are built once per media item and reused for every frame and
    /// every clip cut from it, so this is a no-op on all but the first frame
    /// after a waveform job finishes. It runs before painting because building
    /// a texture needs the context, and painting only reads it.
    fn prepare_waveforms(
        &mut self,
        ctx: &Context,
        project: &Project,
        sequence: &Sequence,
        layout: PanelLayout,
    ) {
        if self.waveforms.is_empty() {
            return;
        }
        let tracks = self.visible_tracks(layout.content.height(), sequence.tracks.len());
        let mut prepared: Vec<sub_model::MediaId> = Vec::new();
        for index in tracks {
            let (Some(track), Some(placements)) =
                (sequence.tracks.get(index), self.layouts.get(index))
            else {
                continue;
            };
            for placement in self.view.visible_clips(placements) {
                let Some(clip) = track
                    .items
                    .get(placement.item_index)
                    .and_then(sub_model::TrackItem::as_clip)
                else {
                    continue;
                };
                if prepared.contains(&clip.media) || !draws_waveform(project, clip) {
                    continue;
                }
                prepared.push(clip.media);
                self.waveforms.prepare(ctx, clip.media);
            }
        }
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
    ) -> HeaderColumnOutcome {
        let mut outcome = HeaderColumnOutcome::default();
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
            let raised = self.header_state.ui(&mut column, rect, track, index, count);
            outcome.actions.extend(raised.action);
            outcome.begin = outcome.begin.or(raised.begin);
            outcome.commit |= raised.commit;
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
                outcome.actions.push(action);
            }
        }
        outcome
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
        painter: &Painter,
        layout: &PanelLayout,
        visuals: &Visuals,
        project: &Project,
        sequence: &Sequence,
        strips: &mut StripPainter<'_>,
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
                self.paint_clips(&lanes, lane, index, track, project, strips);
            }
        }
    }

    /// Paints the clips of one track that the viewport touches.
    fn paint_clips(
        &self,
        painter: &Painter,
        lane: Rect,
        index: &TrackLayout,
        track: &Track,
        project: &Project,
        strips: &mut StripPainter<'_>,
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
            let kind = ClipMediaKind::of(media);
            let dimmed = !clip_edits_allowed(track);
            paint_clip_body(painter, rect, kind, dimmed);
            let strip = paint_clip_strip(painter, rect, clip, kind, dimmed, strips);
            self.paint_waveform(painter, rect, clip, kind, dimmed);
            paint_clip_decoration(
                painter,
                rect,
                clip,
                kind,
                TrimmedEdges::of(clip, media),
                dimmed,
                strip,
            );
            let item = ClipRef::new(track.id, clip.id);
            // An audio clip wears its fades: the ramps say how long they are,
            // and the grips at their tops are what a drag takes hold of.
            if matches!(track.kind, TrackKind::Audio) && !dimmed {
                let dragged = self.fade_preview().filter(|edit| edit.target == item);
                let (head, tail) = self.faded_lengths(clip, dragged);
                let hovered = self
                    .hovered_fade
                    .filter(|(over, _)| *over == item)
                    .map(|(_, edge)| edge);
                paint_fades(
                    painter,
                    rect,
                    self.pixels_of(head),
                    self.pixels_of(tail),
                    hovered,
                );
            }
            if self.selection.contains(item) {
                paint_selected(painter, rect, self.drag_refusal().is_some());
            }
            // Handles are drawn on the clip the pointer is over — the one a
            // press would trim — and nowhere else, so a timeline at rest is
            // not covered in grips.
            if !dimmed && let Some((_, edge)) = self.hovered_trim.filter(|(held, _)| *held == item)
            {
                paint_trim_handles(painter, rect, edge);
            }
        }
    }

    /// How long `clip`'s two fades are, with the drag in progress folded in.
    ///
    /// The ramp under the pointer follows the drag rather than the project, so
    /// what is painted is the fade the release will commit.
    fn faded_lengths(
        &self,
        clip: &Clip,
        dragged: Option<&FadeEdit>,
    ) -> (RationalTime, RationalTime) {
        let rate = self.view.rate();
        let head = clip.fade_in.rescaled_to(rate);
        let tail = clip.fade_out.rescaled_to(rate);
        match dragged {
            Some(edit) => match edit.edge {
                FadeEdge::In => (edit.duration, tail),
                FadeEdge::Out => (head, edit.duration),
            },
            None => (head, tail),
        }
    }

    /// How wide `duration` is on screen, in points.
    fn pixels_of(&self, duration: RationalTime) -> f32 {
        let rate = self.view.rate();
        self.view.pixel_of(duration.rescaled_to(rate))
            - self.view.pixel_of(RationalTime::zero(rate))
    }

    /// Draws the waveform strip inside one clip's rectangle.
    ///
    /// The texture is a picture of the whole source, so the clip draws the
    /// part of it its source range covers; nothing is rebuilt when a clip is
    /// trimmed, split or scrolled.
    fn paint_waveform(
        &self,
        painter: &eframe::egui::Painter,
        rect: Rect,
        clip: &Clip,
        kind: ClipMediaKind,
        dimmed: bool,
    ) {
        if rect.width() < MIN_WAVEFORM_WIDTH_PX {
            return;
        }
        let Some(waveform) = self.waveforms.waveform(clip.media) else {
            return;
        };
        let Some(texture) = self.waveforms.texture(clip.media) else {
            return;
        };
        let strip = Rect::from_min_max(
            pos2(rect.left() + TRIM_BAR_WIDTH, rect.top() + WAVEFORM_INSET),
            pos2(
                rect.right() - TRIM_BAR_WIDTH,
                rect.bottom() - WAVEFORM_INSET,
            ),
        );
        if strip.width() <= 0.0 || strip.height() <= 0.0 {
            return;
        }
        painter.image(
            texture.id(),
            strip,
            waveform.uv_of(clip.source_range),
            dim(waveform_color(kind), dimmed),
        );
    }
}

/// What one frame of the lane gestures produced.
///
/// The panel folds this into its [`TimelineResponse`]; it exists so the
/// gesture code has one thing to return rather than a tuple of three.
/// What one frame of the bin drag produced.
///
/// Empty on every frame but the one the button comes up on, where it carries
/// either the edit the drop asks for or the reason the track refused it.
#[derive(Debug, Default)]
struct DropOutcome {
    /// The edit a released drop asks for.
    edit: Option<PlannedEdit>,
    /// Why the released drop could not become an edit.
    refused: Option<SourceRefusal>,
}

/// Where an edge gesture — a trim or a fade — has hold of a clip this frame.
///
/// The two drags carry the same things, so they travel together rather than as
/// a row of loose arguments.
#[derive(Debug, Clone, Copy)]
struct EdgeGrip {
    /// Where the drag was pressed.
    origin: Pos2,
    /// Where the pointer is now.
    pos: Pos2,
    /// The clip the gesture has hold of.
    target: ClipRef,
    /// Whether the button is still down.
    down: bool,
    /// Whether the ripple modifier is held, which only a trim reads.
    ripple: bool,
}

/// What one painted frame of the header column raised.
///
/// The actions are in the order they were raised; the group brackets are the
/// header's, so a gain drag across several frames is one undo entry.
#[derive(Debug, Default)]
struct HeaderColumnOutcome {
    /// The actions the headers raised this frame.
    actions: Vec<TrackAction>,
    /// The label of the undo group a gesture opened this frame.
    begin: Option<String>,
    /// Whether a gesture ended this frame.
    commit: bool,
}

#[derive(Debug, Default)]
struct LaneOutcome {
    /// Whether the selection changed this frame.
    selection_changed: bool,
    /// The move a released drag asks for.
    clip_move: Option<MoveGroup>,
    /// Why the drag under the pointer cannot become an edit.
    refused: Option<MoveRefusal>,
    /// The trim a released edge drag asks for.
    clip_trim: Option<TrimGroup>,
    /// Why the trim under the pointer cannot become an edit.
    trim_refused: Option<TrimRefusal>,
    /// The fade a released handle drag asks for.
    clip_fade: Option<FadeEdit>,
    /// Why the fade under the pointer cannot become an edit.
    fade_refused: Option<FadeRefusal>,
    /// The cut a razor click asks for.
    clip_split: Option<SplitGroup>,
    /// Why the cut under the pointer cannot become an edit.
    split_refused: Option<SplitRefusal>,
}

/// Paints the grips a trim drag takes hold of, at both ends of a clip.
///
/// The edge under the pointer is painted at full strength and the other at
/// [`TRIM_HANDLE_ALPHA`], so which end a press will grab is visible before the
/// button goes down. A clip too narrow to hold two handles and still show
/// anything between them gets none: on one that small the whole rectangle is
/// the edge, and a press anywhere in it trims.
fn paint_trim_handles(painter: &Painter, rect: Rect, hovered: TrimEdge) {
    if rect.width() < MIN_HANDLE_WIDTH_PX {
        return;
    }
    let radius = CornerRadius::same(3);
    for edge in [TrimEdge::In, TrimEdge::Out] {
        let grip = match edge {
            TrimEdge::In => {
                Rect::from_min_max(rect.min, pos2(rect.left() + TRIM_HANDLE_PX, rect.bottom()))
            }
            TrimEdge::Out => {
                Rect::from_min_max(pos2(rect.right() - TRIM_HANDLE_PX, rect.top()), rect.max)
            }
        };
        let color = if hovered == edge {
            SELECTION_COLOR
        } else {
            tint(SELECTION_COLOR, TRIM_HANDLE_ALPHA)
        };
        painter.rect_filled(grip, radius, color);
    }
}

/// Draws an audio clip's two fade ramps and the grips that drag them.
///
/// A fade of nothing still draws its grip, on the clip's own edge, so there is
/// always something to take hold of; the ramp itself only appears once the
/// fade is long enough to see.
fn paint_fades(painter: &Painter, rect: Rect, head: f32, tail: f32, hovered: Option<FadeEdge>) {
    if rect.width() < MIN_FADE_WIDTH_PX {
        return;
    }
    let head_x = (rect.left() + head.max(0.0)).min(rect.right());
    let tail_x = (rect.right() - tail.max(0.0)).max(rect.left());
    let ramp = Stroke::new(1.0, tint(FADE_COLOR, FADE_ALPHA));
    if head_x > rect.left() + 1.0 {
        painter.line_segment(
            [pos2(rect.left(), rect.bottom()), pos2(head_x, rect.top())],
            ramp,
        );
    }
    if tail_x < rect.right() - 1.0 {
        painter.line_segment(
            [pos2(tail_x, rect.top()), pos2(rect.right(), rect.bottom())],
            ramp,
        );
    }
    for (edge, x) in [(FadeEdge::In, head_x), (FadeEdge::Out, tail_x)] {
        let color = if hovered == Some(edge) {
            FADE_COLOR
        } else {
            tint(FADE_COLOR, FADE_ALPHA)
        };
        let grip = Rect::from_min_size(
            pos2(
                (x - FADE_GRIP_PX / 2.0)
                    .max(rect.left())
                    .min(rect.right() - FADE_GRIP_PX),
                rect.top(),
            ),
            Vec2::splat(FADE_GRIP_PX),
        );
        painter.rect_filled(grip, CornerRadius::same(1), color);
    }
}

/// Marks a selected clip, and says when its drag is being refused.
///
/// A refused drag washes the clip in [`REFUSED_COLOR`] as well as outlining
/// it, so it is obvious before the button comes up that letting go will do
/// nothing (a locked track, or a landing before the head of the sequence).
fn paint_selected(painter: &Painter, rect: Rect, refused: bool) {
    let radius = CornerRadius::same(3);
    let color = if refused {
        REFUSED_COLOR
    } else {
        SELECTION_COLOR
    };
    if refused {
        painter.rect_filled(rect, radius, tint(REFUSED_COLOR, REFUSED_ALPHA));
    }
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(SELECTION_WIDTH, color),
        StrokeKind::Inside,
    );
}

/// `color` at `alpha`, for the washes and fills the gestures paint.
fn tint(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// The two values in ascending order.
fn ordered(left: f32, right: f32) -> (f32, f32) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

/// Whether a clip's media has sound to draw.
fn draws_waveform(project: &Project, clip: &Clip) -> bool {
    project
        .media_item(clip.media)
        .and_then(|media| media.info.as_ref())
        .is_some_and(sub_model::StreamInfo::has_audio)
}

/// The colour a waveform strip is tinted with: the clip's own outline, kept
/// translucent so the clip body still reads as its kind.
fn waveform_color(kind: ClipMediaKind) -> Color32 {
    let outline = kind.outline();
    Color32::from_rgba_unmultiplied(outline.r(), outline.g(), outline.b(), WAVEFORM_ALPHA)
}

/// What a painted frame needs to turn strips into pictures: the egui context
/// the textures live in, how many pixels one point is, and the cache itself.
struct StripPainter<'a> {
    ctx: Context,
    pixels_per_point: f32,
    cache: &'a mut ThumbnailCache,
}

/// Paints one clip's body colour.
fn paint_clip_body(painter: &Painter, rect: Rect, kind: ClipMediaKind, dimmed: bool) {
    painter.rect_filled(rect, CornerRadius::same(3), dim(kind.fill(), dimmed));
}

/// Paints the thumbnail strip inside one clip rectangle, and says whether it
/// painted anything.
///
/// Only picture clips get a strip, and only when the clip is big enough for a
/// tile to be worth looking at. A tile whose texture is not resident yet is
/// left as body colour: the cache uploads a bounded number of textures per
/// painted frame (see [`crate::thumbnails`]), so a scroll through a long
/// sequence fills in over a frame or two instead of stalling on one.
fn paint_clip_strip(
    painter: &Painter,
    rect: Rect,
    clip: &Clip,
    kind: ClipMediaKind,
    dimmed: bool,
    strips: &mut StripPainter<'_>,
) -> bool {
    if !matches!(kind, ClipMediaKind::Video | ClipMediaKind::Still) {
        return false;
    }
    if rect.width() < MIN_STRIP_WIDTH_PX || rect.height() < MIN_STRIP_HEIGHT_PX {
        return false;
    }
    strips.cache.want(clip.media);
    // Which picture each tile shows is worked out first, so the strip is no
    // longer borrowed when the textures are asked for.
    let mut wanted = [0_usize; MAX_TILES_PER_CLIP];
    let Some(strip) = strips.cache.strip(clip.media) else {
        return false;
    };
    let Some(first) = strip.frames().first() else {
        return false;
    };
    let tiles = strip_tiles(rect.width(), rect.height(), first.width, first.height);
    if tiles.count == 0 {
        return false;
    }
    for (index, slot) in wanted.iter_mut().enumerate().take(tiles.count) {
        let time = tile_time(clip.source_range, index, tiles.count);
        *slot = strip.frame_at(time).map_or(0, |frame| frame.index);
    }
    let bucket = ZoomBucket::for_tile(
        tiles.tile_width * strips.pixels_per_point,
        rect.height() * strips.pixels_per_point,
    );

    let ctx = strips.ctx.clone();
    let tint = dim(Color32::WHITE, dimmed);
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let clipped = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let mut drew_a_tile = false;
    for (index, frame) in wanted.iter().enumerate().take(tiles.count) {
        #[allow(
            clippy::cast_precision_loss,
            reason = "the tile index is at most MAX_TILES_PER_CLIP"
        )]
        let left = rect.left() + index as f32 * tiles.tile_width;
        if left >= rect.right() {
            break;
        }
        let Some(texture) = strips.cache.texture(&ctx, clip.media, bucket, *frame) else {
            continue;
        };
        let tile = Rect::from_min_max(
            pos2(left, rect.top()),
            pos2(left + tiles.tile_width, rect.bottom()),
        );
        clipped.image(texture.id(), tile, uv, tint);
        drew_a_tile = true;
    }
    drew_a_tile
}

/// Paints what sits over one clip's body: its outline, its trimmed edges and
/// its name.
fn paint_clip_decoration(
    painter: &Painter,
    rect: Rect,
    clip: &Clip,
    kind: ClipMediaKind,
    trimmed: TrimmedEdges,
    dimmed: bool,
    over_strip: bool,
) {
    let radius = CornerRadius::same(3);
    let outline = dim(kind.outline(), dimmed);
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
        if over_strip {
            // A name over a picture needs something to sit on, whatever the
            // shot under it happens to be.
            painter.rect_filled(
                Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 15.0)).intersect(rect),
                CornerRadius::ZERO,
                Color32::from_black_alpha(NAME_PLATE_ALPHA),
            );
        }
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

/// A position in points as the whole number of pixels it falls in.
///
/// Rounding down rather than to nearest is what makes a lane index the lane
/// the point is actually inside, whatever fraction of it the point is at.
#[allow(
    clippy::cast_possible_truncation,
    reason = "an absurd position is clamped before it is narrowed"
)]
fn floor_px(value: f32) -> i64 {
    value.clamp(-1.0e9, 1.0e9).floor() as i64
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
    use crate::thumbnails::DEFAULT_UPLOADS_PER_FRAME;
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

    /// Paints one frame of `panel` into a headless egui context.
    fn paint_once(
        ctx: &eframe::egui::Context,
        panel: &mut TimelinePanel,
        project: &Project,
        sequence: &Sequence,
    ) {
        let input = eframe::egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(900.0, 400.0))),
            ..eframe::egui::RawInput::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            panel.ui(ui, project, sequence);
        });
        // There is no renderer behind this context to apply the deltas the
        // thumbnail uploads produced.
        output.textures_delta.clear();
    }

    /// A project holding one audio media item lasting `source_duration`
    /// frames, and a sequence whose one audio track carries one clip of it.
    fn audio_project(source_duration: i64, clip_range: TimeRange) -> (Project, Sequence) {
        let mut project = Project::new("test");
        let item = media(ClipMediaKind::Audio, Some(source_duration));
        let media_id = item.id;
        project.media.push(item);
        let mut track = Track::new("A1", TrackKind::Audio);
        track
            .items
            .push(TrackItem::Clip(Clip::new("take", media_id, clip_range)));
        let mut sequence = Sequence::new("seq", SequenceSettings::default());
        sequence.tracks.push(track);
        (project, sequence)
    }

    /// Stereo peaks at 48 kHz, 512 audio frames a peak.
    fn peaks(buckets: usize) -> crate::waveform::ClipWaveform {
        let peaks = (0..buckets)
            .flat_map(|_| {
                [
                    sub_media::Peak {
                        min: -20_000,
                        max: 20_000,
                    },
                    sub_media::Peak {
                        min: -10_000,
                        max: 10_000,
                    },
                ]
            })
            .collect();
        crate::waveform::ClipWaveform::new(48_000, 2, 512, buckets as u64 * 512, peaks)
    }

    /// Paints one frame of the panel headlessly and returns what it drew.
    fn painted(
        panel: &mut TimelinePanel,
        project: &Project,
        sequence: &Sequence,
    ) -> Vec<eframe::egui::epaint::ClippedShape> {
        let ctx = eframe::egui::Context::default();
        paint_one_frame(&ctx, panel, project, sequence)
    }

    /// Paints one frame with a caller's context, so two frames can share one.
    fn paint_one_frame(
        ctx: &eframe::egui::Context,
        panel: &mut TimelinePanel,
        project: &Project,
        sequence: &Sequence,
    ) -> Vec<eframe::egui::epaint::ClippedShape> {
        let input = eframe::egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(800.0, 400.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            panel.ui(ui, project, sequence);
        });
        // Nothing here paints, so the textures the frame uploaded are dropped
        // by hand rather than handed to a renderer.
        output.textures_delta.clear();
        output.shapes
    }

    /// The meshes drawn with one texture, which is how a waveform strip
    /// reaches the screen.
    fn meshes_with(
        shapes: &[eframe::egui::epaint::ClippedShape],
        texture: eframe::egui::TextureId,
    ) -> Vec<Rect> {
        shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                eframe::egui::Shape::Mesh(mesh) if mesh.texture_id == texture => {
                    Some(mesh.calc_bounds())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn strip_tiles_keep_the_thumbnail_aspect_and_are_capped() {
        // A 16:9 thumbnail in a 48-point lane is 85 points wide, so a
        // 200-point clip shows three tiles, the last one cut short.
        let tiles = strip_tiles(200.0, 48.0, 320, 180);
        assert!((tiles.tile_width - 48.0 * 16.0 / 9.0).abs() < 0.01);
        assert_eq!(tiles.count, 3);

        // A clip too small to say anything with a picture gets no tiles.
        assert_eq!(strip_tiles(10.0, 48.0, 320, 180).count, 0);
        assert_eq!(strip_tiles(200.0, 6.0, 320, 180).count, 0);
        assert_eq!(strip_tiles(f32::NAN, 48.0, 320, 180).count, 0);

        // Past the cap the tiles widen rather than multiply, and they still
        // cover the clip exactly.
        let long = strip_tiles(100_000.0, 48.0, 320, 180);
        assert_eq!(long.count, MAX_TILES_PER_CLIP);
        #[allow(clippy::cast_precision_loss, reason = "the cap is 64")]
        let covered = long.tile_width * long.count as f32;
        assert!((covered - 100_000.0).abs() < 1.0);

        // A thumbnail with no size recorded falls back on 16:9 rather than
        // dividing by zero.
        assert_eq!(
            strip_tiles(200.0, 48.0, 0, 0).count,
            strip_tiles(200.0, 48.0, 16, 9).count
        );
    }

    #[test]
    fn a_video_clip_paints_its_strip_and_a_clip_too_narrow_for_one_does_not() {
        let dir = crate::thumbnails::fixtures::temp_dir("panel-strip");
        let (project, sequence) = project_with(3, 240);
        let media = project.media[0].id;
        let ctx = eframe::egui::Context::default();
        let mut panel = TimelinePanel::new(RATE);
        panel.sync(&sequence, 1);
        panel.view_mut().set_zoom(zoom(2, 1));

        // With no strip yet the clips still paint, and the media is reported
        // for the caller to queue a thumbnail job against.
        paint_once(&ctx, &mut panel, &project, &sequence);
        assert_eq!(panel.thumbnails().stats().uploads, 0);
        assert_eq!(panel.thumbnails_mut().take_missing(), vec![media]);

        let strip = crate::thumbnails::fixtures::fixture_strip(&dir, 6, (64, 36));
        panel.thumbnails_mut().insert_strip(media, strip);
        paint_once(&ctx, &mut panel, &project, &sequence);
        assert!(
            panel.thumbnails().stats().uploads > 0,
            "a video clip wide enough for a tile shows one"
        );
        assert!(panel.thumbnails().bytes() > 0);
        assert!(
            panel.thumbnails_mut().take_missing().is_empty(),
            "media with a strip is not asked for again"
        );

        // Zoomed out until a clip is a few points wide, a picture would say
        // less than the clip's own colour: no tiles, no textures.
        let uploaded = panel.thumbnails().stats().uploads;
        panel.view_mut().set_zoom(zoom(1, 16));
        paint_once(&ctx, &mut panel, &project, &sequence);
        assert_eq!(panel.thumbnails().stats().uploads, uploaded);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scrolling_a_two_hundred_clip_sequence_is_bounded_work_per_frame() {
        let dir = crate::thumbnails::fixtures::temp_dir("panel-scroll");
        let (project, sequence) = project_with(200, 240);
        let media = project.media[0].id;
        let ctx = eframe::egui::Context::default();
        let mut panel = TimelinePanel::new(RATE);
        panel.sync(&sequence, 1);
        panel.view_mut().set_zoom(zoom(2, 1));
        panel.thumbnails_mut().insert_strip(
            media,
            crate::thumbnails::fixtures::fixture_strip(&dir, 12, (64, 36)),
        );
        // A budget of a few tiles, so eviction runs throughout rather than
        // only at the end.
        let budget = panel.thumbnails().config().budget_bytes;
        assert!(budget > 0);

        let started = std::time::Instant::now();
        let mut previous = 0;
        for frame in 0..120 {
            panel.view_mut().scroll_by(37);
            paint_once(&ctx, &mut panel, &project, &sequence);
            let uploads = panel.thumbnails().stats().uploads;
            assert!(
                uploads - previous <= DEFAULT_UPLOADS_PER_FRAME,
                "frame {frame} uploaded {} textures",
                uploads - previous
            );
            previous = uploads;
            assert!(
                panel.thumbnails().bytes() <= budget,
                "frame {frame} is over the texture budget"
            );
        }
        // Not a benchmark: a bound loose enough for a loaded machine, tight
        // enough to catch a paint that decodes a strip per clip per frame.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "120 painted frames of 200 clips took {:?}",
            started.elapsed()
        );
        assert!(
            panel.thumbnails().stats().hits > 0,
            "a scrolled strip is re-used, not rebuilt"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_audio_clip_is_drawn_with_its_cached_waveform_texture() {
        let (project, sequence) = audio_project(240, range(0, 240));
        let media_id = project.media[0].id;
        let mut panel = TimelinePanel::new(RATE);
        panel.sync(&sequence, 1);
        panel.waveforms_mut().insert(media_id, peaks(470));

        let ctx = eframe::egui::Context::default();
        let shapes = paint_one_frame(&ctx, &mut panel, &project, &sequence);
        assert_eq!(
            panel.waveforms().textures(),
            1,
            "the strip is uploaded once, on the frame that first draws it"
        );
        let texture = panel
            .waveforms()
            .texture(media_id)
            .expect("a waveform texture")
            .id();
        let drawn = meshes_with(&shapes, texture);
        assert_eq!(drawn.len(), 1, "the clip draws its waveform exactly once");

        // The strip sits inside the clip rectangle, not over the lane beside
        // it: the clip runs from sequence zero to frame 240.
        let lane_left = panel.layout().expect("a layout").content.left();
        let clip_right = lane_left + panel.view().pixel_of(frames(240));
        assert!(drawn[0].left() >= lane_left);
        assert!(drawn[0].right() <= clip_right + 1.0);

        // A second frame reuses the texture rather than uploading another.
        let again = paint_one_frame(&ctx, &mut panel, &project, &sequence);
        assert_eq!(panel.waveforms().textures(), 1);
        assert_eq!(
            panel
                .waveforms()
                .texture(media_id)
                .expect("a waveform texture")
                .id(),
            texture,
            "the cached texture survives from frame to frame"
        );
        assert_eq!(meshes_with(&again, texture).len(), 1);
    }

    #[test]
    fn a_clip_with_no_peaks_yet_is_painted_without_a_waveform() {
        let (project, sequence) = audio_project(240, range(0, 240));
        let mut panel = TimelinePanel::new(RATE);
        panel.sync(&sequence, 1);
        // Nothing was ever handed to the cache: the panel paints the clip and
        // asks for no texture at all.
        let shapes = painted(&mut panel, &project, &sequence);
        assert!(panel.waveforms().is_empty());
        assert_eq!(panel.waveforms().textures(), 0);
        assert!(!shapes.is_empty(), "the clip itself is still painted");
    }

    #[test]
    fn a_silent_media_item_is_never_uploaded_even_when_peaks_are_offered() {
        // A still has no audio, so the panel must not ask for its waveform
        // even when one has somehow been cached for it.
        let mut project = Project::new("test");
        let item = media(ClipMediaKind::Still, None);
        let media_id = item.id;
        project.media.push(item);
        let mut track = Track::new("V1", TrackKind::Video);
        track
            .items
            .push(TrackItem::Clip(Clip::new("still", media_id, range(0, 48))));
        let mut sequence = Sequence::new("seq", SequenceSettings::default());
        sequence.tracks.push(track);

        let mut panel = TimelinePanel::new(RATE);
        panel.sync(&sequence, 1);
        panel.waveforms_mut().insert(media_id, peaks(10));
        let _ = painted(&mut panel, &project, &sequence);
        assert_eq!(
            panel.waveforms().textures(),
            0,
            "a media item with no sound costs no texture"
        );
    }

    #[test]
    fn a_trimmed_clip_samples_only_its_own_part_of_the_texture() {
        // The middle two seconds of a ten-second source.
        let waveform = peaks(938);
        let uv = waveform.uv_of(range(96, 48));
        assert!((uv.left() - 0.4).abs() < 0.01, "{uv:?}");
        assert!((uv.right() - 0.6).abs() < 0.01, "{uv:?}");
    }
}
