//! The timeline view model: pixels to time, time to pixels, and which clips
//! a viewport actually touches.
//!
//! A sequence can hold hundreds of clips, so the timeline panel paints only
//! what is visible (docs/PLAN.md §5.7). Everything the panel needs to decide
//! that lives here, deliberately free of egui: the mapping between screen
//! pixels and sequence time ([`TimelineView`]), the zoom ladder that mapping
//! rides on ([`ZoomLevel`]), and a per-track index that answers "which clips
//! intersect this span" in logarithmic time ([`TrackLayout`]).
//!
//! No time is ever a float. Zoom is an exact rational number of pixels per
//! frame and every conversion is integer arithmetic; a float appears only at
//! the painting boundary, in [`TimelineView::pixel_of`], because a `Painter`
//! takes float coordinates.
//!
//! ```
//! use sub_time::{Rational, RationalTime};
//! use sub_ui::timeline::{TimelineView, ZoomLevel};
//!
//! let mut view = TimelineView::new(Rational::FPS_24);
//! view.set_width_px(1920);
//! view.set_zoom(ZoomLevel::new(Rational::new(8, 1).unwrap()).unwrap());
//!
//! // Eight pixels per frame, so the 1920-pixel viewport shows 240 frames.
//! let visible = view.visible_range();
//! assert_eq!(visible.start(), RationalTime::new(0, Rational::FPS_24));
//! assert_eq!(visible.duration(), RationalTime::new(240, Rational::FPS_24));
//! assert_eq!(view.time_at_pixel(16), RationalTime::new(2, Rational::FPS_24));
//! ```

use sub_core::{SubError, SubResult};
use sub_model::ids::ClipId;
use sub_model::track::Track;
use sub_time::{Rational, RationalTime, Rounding, TimeRange};

use crate::codes;

/// Floored division, for a strictly positive divisor.
const fn div_floor(numerator: i128, denominator: i128) -> i128 {
    numerator.div_euclid(denominator)
}

/// Division rounded towards positive infinity, for a strictly positive
/// divisor.
const fn div_ceil(numerator: i128, denominator: i128) -> i128 {
    -((-numerator).div_euclid(denominator))
}

/// Narrows to a pixel or frame count, saturating rather than wrapping.
fn saturate_i64(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value.is_negative() {
        i64::MIN
    } else {
        i64::MAX
    })
}

/// The greatest common divisor, used to reduce a zoom fraction before it is
/// narrowed back into a [`Rational`].
const fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// How many pixels one frame of the sequence occupies.
///
/// The scale is an exact fraction rather than a float so that the pixel/time
/// mapping is reversible and never drifts. A [`Rational`] is reused for it:
/// the type is a reduced, strictly positive fraction, which is exactly what a
/// scale factor needs, even though here it counts pixels per frame rather
/// than units per second.
///
/// The ladder spans the whole useful range demanded by docs/PLAN.md §5.7:
/// [`ZoomLevel::MIN`] fits a sequence of over four billion frames into a
/// single pixel column's worth of scrolling, and [`ZoomLevel::MAX`] gives a
/// single frame more pixels than any viewport is wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoomLevel {
    /// Pixels per frame; between [`ZoomLevel::MIN`] and [`ZoomLevel::MAX`].
    pixels_per_frame: Rational,
}

impl ZoomLevel {
    /// The furthest out: one pixel covers 65536 frames, so a forty-five minute
    /// sequence at 24 fps is about a pixel wide.
    pub const MIN: Self = Self {
        pixels_per_frame: match Rational::new(1, 65536) {
            Some(rate) => rate,
            None => unreachable!(),
        },
    };

    /// The furthest in: one frame spans 1024 pixels, wider than any viewport,
    /// which is as far as "zoom to a single frame" can usefully go.
    pub const MAX: Self = Self {
        pixels_per_frame: match Rational::new(1024, 1) {
            Some(rate) => rate,
            None => unreachable!(),
        },
    };

    /// One pixel per frame: the default a fresh view starts at.
    pub const ONE: Self = Self {
        pixels_per_frame: Rational::ONE,
    };

    /// Creates a zoom level of `pixels_per_frame` pixels per frame.
    ///
    /// # Errors
    ///
    /// Returns `ui.invalid_zoom` if the scale falls outside
    /// [`ZoomLevel::MIN`]..=[`ZoomLevel::MAX`]. Use [`ZoomLevel::clamped`]
    /// where an out-of-range scale should be pinned to the end of the ladder
    /// instead, as it should be when the user keeps scrolling the wheel.
    pub fn new(pixels_per_frame: Rational) -> SubResult<Self> {
        let candidate = Self { pixels_per_frame };
        if candidate < Self::MIN || candidate > Self::MAX {
            return Err(SubError::new(
                codes::INVALID_ZOOM,
                "zoom must be between 1/65536 and 1024 pixels per frame",
            )
            .with_detail("numerator", pixels_per_frame.numerator())
            .with_detail("denominator", pixels_per_frame.denominator()));
        }
        Ok(candidate)
    }

    /// Creates a zoom level, pinning an out-of-range scale to the nearest end
    /// of the ladder.
    #[must_use]
    pub fn clamped(pixels_per_frame: Rational) -> Self {
        Self::from_parts_clamped(
            u128::from(pixels_per_frame.numerator()),
            u128::from(pixels_per_frame.denominator()),
        )
    }

    /// The zoom that fits `frames` frames into `width_px` pixels, clamped to
    /// the ladder.
    ///
    /// An empty viewport or an empty sequence has nothing to fit, and yields
    /// [`ZoomLevel::ONE`].
    #[must_use]
    pub fn fit(frames: i64, width_px: u32) -> Self {
        if frames <= 0 || width_px == 0 {
            return Self::ONE;
        }
        Self::from_parts_clamped(u128::from(width_px), frames.unsigned_abs().into())
    }

    /// This zoom doubled `steps` times, or halved `-steps` times, clamped to
    /// the ladder.
    #[must_use]
    pub fn zoomed(self, steps: i32) -> Self {
        // Beyond 32 steps either end of the ladder is already reached, so
        // clamping the exponent keeps the shift in range without changing the
        // result.
        let steps = steps.clamp(-32, 32);
        let shift = steps.unsigned_abs();
        let mut numerator = u128::from(self.pixels_per_frame.numerator());
        let mut denominator = u128::from(self.pixels_per_frame.denominator());
        if steps >= 0 {
            numerator <<= shift;
        } else {
            denominator <<= shift;
        }
        Self::from_parts_clamped(numerator, denominator)
    }

    /// The scale, in pixels per frame.
    #[must_use]
    pub const fn pixels_per_frame(self) -> Rational {
        self.pixels_per_frame
    }

    /// True if one frame is at least one pixel wide, the point past which
    /// individual frames can be labelled.
    #[must_use]
    pub fn is_frame_resolvable(self) -> bool {
        self.pixels_per_frame.numerator() >= self.pixels_per_frame.denominator()
    }

    /// Builds a zoom from an unreduced fraction, clamping it to the ladder.
    fn from_parts_clamped(numerator: u128, denominator: u128) -> Self {
        if denominator == 0 || numerator == 0 {
            return Self::MIN;
        }
        if cross_le(numerator, denominator, Self::MIN) {
            return Self::MIN;
        }
        if cross_ge(numerator, denominator, Self::MAX) {
            return Self::MAX;
        }
        let divisor = gcd(numerator, denominator);
        let numerator = numerator / divisor;
        let denominator = denominator / divisor;
        // Within the ladder and reduced, both parts fit a `u32` for every
        // fraction the UI can produce; anything pathological keeps MIN rather
        // than inventing a scale.
        match (u32::try_from(numerator), u32::try_from(denominator)) {
            (Ok(numerator), Ok(denominator)) => Rational::new(numerator, denominator)
                .map_or(Self::MIN, |pixels_per_frame| Self { pixels_per_frame }),
            _ => Self::MIN,
        }
    }
}

/// True if `numerator / denominator` is at most `other`'s scale.
fn cross_le(numerator: u128, denominator: u128, other: ZoomLevel) -> bool {
    numerator * u128::from(other.pixels_per_frame.denominator())
        <= denominator * u128::from(other.pixels_per_frame.numerator())
}

/// True if `numerator / denominator` is at least `other`'s scale.
fn cross_ge(numerator: u128, denominator: u128, other: ZoomLevel) -> bool {
    numerator * u128::from(other.pixels_per_frame.denominator())
        >= denominator * u128::from(other.pixels_per_frame.numerator())
}

impl Default for ZoomLevel {
    fn default() -> Self {
        Self::ONE
    }
}

impl PartialOrd for ZoomLevel {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ZoomLevel {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        let mine = u128::from(self.pixels_per_frame.numerator())
            * u128::from(other.pixels_per_frame.denominator());
        let theirs = u128::from(other.pixels_per_frame.numerator())
            * u128::from(self.pixels_per_frame.denominator());
        mine.cmp(&theirs)
    }
}

/// What the timeline panel is looking at: a horizontal scroll position, a
/// zoom level and a viewport width, over a sequence with a fixed timebase.
///
/// The scroll position is kept in pixels rather than in time so that
/// scrolling stays smooth at high zoom, where one pixel is a fraction of a
/// frame. It is measured from sequence time zero and never goes negative.
#[derive(Debug, Clone, Copy)]
pub struct TimelineView {
    /// The sequence timebase every returned time is expressed at.
    rate: Rational,
    /// Pixels per frame.
    zoom: ZoomLevel,
    /// Pixels between sequence time zero and the left edge of the viewport.
    scroll_px: i64,
    /// Viewport width in pixels.
    width_px: u32,
}

impl TimelineView {
    /// A view of a sequence with timebase `rate`, at the origin, one pixel per
    /// frame, with an empty viewport.
    ///
    /// The panel calls [`TimelineView::set_width_px`] every frame with the
    /// width egui gave it.
    #[must_use]
    pub fn new(rate: Rational) -> Self {
        Self {
            rate,
            zoom: ZoomLevel::ONE,
            scroll_px: 0,
            width_px: 0,
        }
    }

    /// The sequence timebase.
    #[must_use]
    pub const fn rate(&self) -> Rational {
        self.rate
    }

    /// The current zoom.
    #[must_use]
    pub const fn zoom(&self) -> ZoomLevel {
        self.zoom
    }

    /// The viewport width in pixels.
    #[must_use]
    pub const fn width_px(&self) -> u32 {
        self.width_px
    }

    /// Pixels between sequence time zero and the left edge of the viewport.
    #[must_use]
    pub const fn scroll_px(&self) -> i64 {
        self.scroll_px
    }

    /// Sets the viewport width, in pixels.
    pub const fn set_width_px(&mut self, width_px: u32) {
        self.width_px = width_px;
    }

    /// Sets the zoom, leaving the left edge where it is.
    ///
    /// Use [`TimelineView::zoom_to`] to zoom around the pointer instead.
    pub fn set_zoom(&mut self, zoom: ZoomLevel) {
        let anchor = self.time_at_pixel(0);
        self.zoom = zoom;
        self.scroll_to(anchor);
    }

    /// Sets the zoom, keeping the instant currently under `anchor_px` under
    /// that same pixel.
    ///
    /// This is what a wheel zoom wants: the frame the pointer is over stays
    /// put while the timeline expands around it.
    pub fn zoom_to(&mut self, zoom: ZoomLevel, anchor_px: i64) {
        let anchor = self.time_at_pixel(anchor_px);
        self.zoom = zoom;
        self.set_scroll_px(self.absolute_pixel_of(anchor).saturating_sub(anchor_px));
    }

    /// Scrolls so the left edge sits at `scroll_px` pixels from time zero,
    /// clamped at the origin.
    pub const fn set_scroll_px(&mut self, scroll_px: i64) {
        self.scroll_px = if scroll_px < 0 { 0 } else { scroll_px };
    }

    /// Scrolls by `delta_px` pixels, clamped at the origin.
    pub const fn scroll_by(&mut self, delta_px: i64) {
        self.set_scroll_px(self.scroll_px.saturating_add(delta_px));
    }

    /// Scrolls so `time` sits at the left edge, clamped at the origin.
    pub fn scroll_to(&mut self, time: RationalTime) {
        self.set_scroll_px(self.absolute_pixel_of(time));
    }

    /// Zooms so that `duration` of sequence exactly fills the viewport, and
    /// returns to the origin.
    ///
    /// This is the "whole sequence" end of the zoom range; the other end is
    /// [`TimelineView::zoom_to_frame`].
    pub fn fit(&mut self, duration: RationalTime) {
        let frames = duration
            .rescaled_to_rounding(self.rate, Rounding::Ceil)
            .value();
        self.zoom = ZoomLevel::fit(frames, self.width_px);
        self.scroll_px = 0;
    }

    /// Zooms all the way in, so a single frame is as wide as it can be, and
    /// puts `time` at the left edge.
    pub fn zoom_to_frame(&mut self, time: RationalTime) {
        self.zoom = ZoomLevel::MAX;
        self.scroll_to(time);
    }

    /// The instant at `px` pixels from the left edge of the viewport.
    ///
    /// A pixel column covers a span of time, and the instant returned is the
    /// start of the frame that column falls in, which is what click-to-seek
    /// wants. Pixels left of the origin yield negative times.
    #[must_use]
    pub fn time_at_pixel(&self, px: i64) -> RationalTime {
        self.time_at_absolute_pixel(i128::from(self.scroll_px) + i128::from(px), Rounding::Floor)
    }

    /// The exact pixel column, measured from sequence time zero, that `time`
    /// starts in.
    ///
    /// Truncated to a whole pixel; [`TimelineView::pixel_of`] keeps the
    /// fractional part for painting.
    #[must_use]
    pub fn absolute_pixel_of(&self, time: RationalTime) -> i64 {
        let (scaled, denominator) = self.scaled(time);
        saturate_i64(div_floor(scaled, denominator))
    }

    /// Where to paint `time`, in pixels from the left edge of the viewport.
    ///
    /// The only float in the view model, and the last step of the
    /// computation: the position is worked out exactly in integers and
    /// divided once, so a clip edge lands on the same subpixel however far
    /// along the timeline it sits.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "painting coordinates are floats; the time arithmetic above them is exact"
    )]
    pub fn pixel_of(&self, time: RationalTime) -> f32 {
        let (scaled, denominator) = self.scaled(time);
        let offset = scaled - i128::from(self.scroll_px) * denominator;
        (offset as f64 / denominator as f64) as f32
    }

    /// The half-open span of sequence time the viewport touches.
    ///
    /// The start is floored and the end ceiled, so a frame only partly on
    /// screen is inside the range: a clip that overlaps it has a visible
    /// edge and must be painted.
    #[must_use]
    pub fn visible_range(&self) -> TimeRange {
        let left = i128::from(self.scroll_px);
        let start = self.time_at_absolute_pixel(left, Rounding::Floor);
        let end = self.time_at_absolute_pixel(left + i128::from(self.width_px), Rounding::Ceil);
        TimeRange::from_start_end(start, end).unwrap_or_else(|| TimeRange::empty_at(start))
    }

    /// The clips of `layout` that the viewport touches, in track order.
    ///
    /// The slice is the whole answer: a clip outside it cannot have a pixel on
    /// screen, so the panel iterates only over what it paints.
    #[must_use]
    pub fn visible_clips<'a>(&self, layout: &'a TrackLayout) -> &'a [ClipPlacement] {
        layout.visible(self.visible_range())
    }

    /// The instant at `px` pixels from sequence time zero, rounded as asked.
    fn time_at_absolute_pixel(&self, px: i128, rounding: Rounding) -> RationalTime {
        let numerator = i128::from(self.zoom.pixels_per_frame.numerator());
        let denominator = i128::from(self.zoom.pixels_per_frame.denominator());
        // px pixels / (numerator/denominator pixels per frame) frames.
        let scaled = px * denominator;
        let frames = match rounding {
            Rounding::Ceil => div_ceil(scaled, numerator),
            _ => div_floor(scaled, numerator),
        };
        RationalTime::new(saturate_i64(frames), self.rate)
    }

    /// `time` in pixels, as an exact fraction: the numerator, and the
    /// denominator it still has to be divided by.
    fn scaled(&self, time: RationalTime) -> (i128, i128) {
        let frames = i128::from(time.rescaled_to(self.rate).value());
        let numerator = i128::from(self.zoom.pixels_per_frame.numerator());
        let denominator = i128::from(self.zoom.pixels_per_frame.denominator());
        (frames * numerator, denominator)
    }
}

/// Where one clip sits in sequence time, and where to find it on its track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipPlacement {
    /// The clip's stable identity.
    pub clip: ClipId,
    /// The clip's position in [`Track::items`], so the panel can reach the
    /// clip itself without searching for it.
    pub item_index: usize,
    /// The span the clip occupies in sequence time.
    pub range: TimeRange,
}

/// A track's clips indexed by the time they occupy, so a viewport query is a
/// binary search rather than a walk.
///
/// Placement on a track is positional — a clip starts where the items before
/// it end — so working out where any clip sits costs a walk of the whole
/// track. Doing that per painted frame is what makes a long timeline crawl;
/// building this index once per edit and querying it per frame is what makes
/// the panel virtualised. A query costs `O(log n)` plus the number of clips
/// it returns, whatever the length of the track.
#[derive(Debug, Clone, Default)]
pub struct TrackLayout {
    /// Every clip on the track, ordered by start, spans never overlapping.
    placements: Vec<ClipPlacement>,
}

impl TrackLayout {
    /// Indexes `track`, whose items are laid out at the sequence timebase
    /// `rate`.
    ///
    /// Rebuild this whenever the track changes; it is a cache of the model,
    /// never a second copy of the truth.
    #[must_use]
    pub fn build(track: &Track, rate: Rational) -> Self {
        let placements = track
            .placements(rate)
            .enumerate()
            .filter_map(|(item_index, (item, range))| {
                item.as_clip().map(|clip| ClipPlacement {
                    clip: clip.id,
                    item_index,
                    range,
                })
            })
            .collect();
        Self { placements }
    }

    /// Every indexed clip, in track order.
    #[must_use]
    pub fn placements(&self) -> &[ClipPlacement] {
        &self.placements
    }

    /// How many clips the track holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.placements.len()
    }

    /// True if the track holds no clips.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.placements.is_empty()
    }

    /// The clips whose spans intersect `range`, as a contiguous slice.
    ///
    /// Intersection is half-open on both sides: a clip ending exactly where
    /// `range` starts, or starting exactly where it ends, has no pixel inside
    /// and is left out. An empty `range` selects nothing.
    #[must_use]
    pub fn visible(&self, range: TimeRange) -> &[ClipPlacement] {
        if range.is_empty() {
            return &[];
        }
        let start = range.start();
        let end = range.end_exclusive();
        let first = self
            .placements
            .partition_point(|placement| placement.range.end_exclusive() <= start);
        let last = self
            .placements
            .partition_point(|placement| placement.range.start() < end);
        &self.placements[first..last.max(first)]
    }

    /// The clip containing `time`, if the track has one there.
    #[must_use]
    pub fn at(&self, time: RationalTime) -> Option<&ClipPlacement> {
        let index = self
            .placements
            .partition_point(|placement| placement.range.end_exclusive() <= time);
        self.placements
            .get(index)
            .filter(|placement| placement.range.contains(time))
    }

    /// The end of the last clip: how much of the track has content.
    ///
    /// `rate` is the timebase of the zero an empty track reports.
    #[must_use]
    pub fn content_duration(&self, rate: Rational) -> RationalTime {
        self.placements.last().map_or_else(
            || RationalTime::zero(rate),
            |last| last.range.end_exclusive(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sub_model::ids::MediaId;
    use sub_model::track::{Clip, Gap, TrackItem, TrackKind};

    const RATE: Rational = Rational::FPS_24;

    fn frames(value: i64) -> RationalTime {
        RationalTime::new(value, RATE)
    }

    fn range(start: i64, duration: i64) -> TimeRange {
        TimeRange::new(frames(start), frames(duration)).unwrap()
    }

    fn clip(duration: i64) -> Clip {
        Clip::new("clip", MediaId::new(), range(0, duration))
    }

    fn track_of(durations: &[i64]) -> Track {
        let mut track = Track::new("V1", TrackKind::Video);
        for &duration in durations {
            track.items.push(TrackItem::Clip(clip(duration)));
        }
        track
    }

    fn view(zoom: ZoomLevel, width_px: u32) -> TimelineView {
        let mut view = TimelineView::new(RATE);
        view.set_width_px(width_px);
        view.set_zoom(zoom);
        view
    }

    fn zoom(numerator: u32, denominator: u32) -> ZoomLevel {
        ZoomLevel::new(Rational::new(numerator, denominator).unwrap()).unwrap()
    }

    #[test]
    fn zoom_ladder_rejects_and_clamps_out_of_range_scales() {
        let too_far_in = Rational::new(2048, 1).unwrap();
        let err = ZoomLevel::new(too_far_in).unwrap_err();
        assert_eq!(err.code.as_str(), "ui.invalid_zoom");
        assert_eq!(ZoomLevel::clamped(too_far_in), ZoomLevel::MAX);

        let too_far_out = Rational::new(1, 131_072).unwrap();
        assert!(ZoomLevel::new(too_far_out).is_err());
        assert_eq!(ZoomLevel::clamped(too_far_out), ZoomLevel::MIN);

        assert_eq!(ZoomLevel::clamped(Rational::ONE), ZoomLevel::ONE);
        assert!(ZoomLevel::new(ZoomLevel::MIN.pixels_per_frame()).is_ok());
        assert!(ZoomLevel::new(ZoomLevel::MAX.pixels_per_frame()).is_ok());
    }

    #[test]
    fn zoom_steps_double_and_halve_and_stop_at_the_ends() {
        assert_eq!(ZoomLevel::ONE.zoomed(1), zoom(2, 1));
        assert_eq!(ZoomLevel::ONE.zoomed(-1), zoom(1, 2));
        assert_eq!(ZoomLevel::ONE.zoomed(10), ZoomLevel::MAX);
        assert_eq!(ZoomLevel::ONE.zoomed(11), ZoomLevel::MAX);
        assert_eq!(ZoomLevel::ONE.zoomed(-16), ZoomLevel::MIN);
        assert_eq!(ZoomLevel::ONE.zoomed(-17), ZoomLevel::MIN);
        assert_eq!(ZoomLevel::MAX.zoomed(i32::MAX), ZoomLevel::MAX);
        assert_eq!(ZoomLevel::MIN.zoomed(i32::MIN), ZoomLevel::MIN);
        // Stepping out and back in returns exactly where it started.
        assert_eq!(zoom(3, 1).zoomed(-5).zoomed(5), zoom(3, 1));
        assert!(ZoomLevel::ONE.is_frame_resolvable());
        assert!(!ZoomLevel::ONE.zoomed(-1).is_frame_resolvable());
    }

    #[test]
    fn pixels_and_time_round_trip_at_one_frame_per_pixel() {
        let view = view(ZoomLevel::ONE, 800);
        for frame in [0, 1, 42, 799, 100_000] {
            assert_eq!(view.time_at_pixel(frame), frames(frame));
            assert_eq!(view.absolute_pixel_of(frames(frame)), frame);
        }
    }

    #[test]
    fn maximum_zoom_resolves_a_single_frame() {
        // 1024 pixels per frame: every pixel of a frame maps back to it, and
        // the next pixel column starts the next frame.
        let view = view(ZoomLevel::MAX, 1920);
        assert_eq!(view.time_at_pixel(0), frames(0));
        assert_eq!(view.time_at_pixel(1023), frames(0));
        assert_eq!(view.time_at_pixel(1024), frames(1));
        assert_eq!(view.absolute_pixel_of(frames(1)), 1024);
        // A viewport 1920 pixels wide shows parts of two frames.
        assert_eq!(view.visible_range(), range(0, 2));
    }

    #[test]
    fn minimum_zoom_holds_a_whole_sequence() {
        // 1/65536 pixels per frame: a pixel is 65536 frames, over 45 minutes
        // of 24 fps material.
        let view = view(ZoomLevel::MIN, 1920);
        assert_eq!(view.time_at_pixel(0), frames(0));
        assert_eq!(view.time_at_pixel(1), frames(65_536));
        assert_eq!(view.absolute_pixel_of(frames(65_535)), 0);
        assert_eq!(view.absolute_pixel_of(frames(65_536)), 1);
        assert_eq!(view.visible_range(), range(0, 1920 * 65_536));
    }

    #[test]
    fn negative_pixels_map_to_negative_time() {
        let view = view(zoom(4, 1), 800);
        assert_eq!(view.time_at_pixel(-1), frames(-1));
        assert_eq!(view.time_at_pixel(-4), frames(-1));
        assert_eq!(view.time_at_pixel(-5), frames(-2));
        assert_eq!(view.absolute_pixel_of(frames(-2)), -8);
    }

    #[test]
    fn fit_shows_the_whole_sequence_and_no_more() {
        let mut view = TimelineView::new(RATE);
        view.set_width_px(1000);
        view.fit(frames(4000));
        assert_eq!(view.zoom(), zoom(1, 4));
        assert_eq!(view.scroll_px(), 0);
        assert_eq!(view.visible_range(), range(0, 4000));

        // A sequence longer than the ladder allows still fits as far as it can.
        view.fit(frames(i64::from(u32::MAX)));
        assert_eq!(view.zoom(), ZoomLevel::MIN);
        // Nothing to fit leaves a usable zoom rather than dividing by zero.
        view.fit(frames(0));
        assert_eq!(view.zoom(), ZoomLevel::ONE);
    }

    #[test]
    fn zoom_to_frame_goes_all_the_way_in() {
        let mut view = TimelineView::new(RATE);
        view.set_width_px(1000);
        view.zoom_to_frame(frames(50));
        assert_eq!(view.zoom(), ZoomLevel::MAX);
        assert_eq!(view.time_at_pixel(0), frames(50));
        assert_eq!(view.visible_range(), range(50, 1));
    }

    #[test]
    fn zooming_keeps_the_time_under_the_anchor_pixel() {
        let anchor_px = 20;
        for steps in [1, 3, 8, -2, -7, -9] {
            let mut view = view(ZoomLevel::ONE, 1000);
            view.set_scroll_px(100_000);
            let before = view.time_at_pixel(anchor_px);
            assert_eq!(before, frames(100_020));

            view.zoom_to(view.zoom().zoomed(steps), anchor_px);
            let after = view.time_at_pixel(anchor_px);
            // The anchor holds to the precision the new zoom can express,
            // which at low zoom is a whole pixel's worth of frames.
            let pixel = view.time_at_pixel(anchor_px + 1) - after;
            let drift = (after - before).checked_abs().unwrap();
            assert!(drift <= pixel, "steps {steps}: drifted {drift} of {pixel}");
        }
    }

    #[test]
    fn scrolling_clamps_at_the_origin() {
        let mut view = view(ZoomLevel::ONE, 100);
        view.scroll_by(-10);
        assert_eq!(view.scroll_px(), 0);
        view.scroll_by(30);
        assert_eq!(view.scroll_px(), 30);
        view.scroll_to(frames(-5));
        assert_eq!(view.scroll_px(), 0);
        view.scroll_to(frames(12));
        assert_eq!(view.scroll_px(), 12);
    }

    #[test]
    fn painting_positions_keep_subpixel_precision() {
        let view = view(zoom(1, 3), 900);
        assert!((view.pixel_of(frames(3)) - 1.0).abs() < f32::EPSILON);
        assert!((view.pixel_of(frames(1)) - 1.0 / 3.0).abs() < 1e-6);
        let mut scrolled = view;
        scrolled.set_scroll_px(10);
        assert!((scrolled.pixel_of(frames(30)) - 0.0).abs() < f32::EPSILON);
        assert!((scrolled.pixel_of(frames(0)) + 10.0).abs() < f32::EPSILON);
    }

    #[test]
    fn times_at_other_rates_are_rescaled_before_mapping() {
        let view = view(ZoomLevel::ONE, 800);
        // 12 frames at 48 fps is 6 frames at 24 fps.
        let at_48 = RationalTime::new(12, Rational::new(48, 1).unwrap());
        assert_eq!(view.absolute_pixel_of(at_48), 6);
    }

    #[test]
    fn empty_viewport_sees_nothing() {
        let view = view(ZoomLevel::ONE, 0);
        let visible = view.visible_range();
        assert!(visible.is_empty());
        let layout = TrackLayout::build(&track_of(&[10, 10]), RATE);
        assert!(view.visible_clips(&layout).is_empty());
    }

    #[test]
    fn layout_indexes_clips_past_gaps_and_reports_their_item_index() {
        let mut track = Track::new("V1", TrackKind::Video);
        track.items.push(TrackItem::Gap(Gap::new(frames(5))));
        track.items.push(TrackItem::Clip(clip(10)));
        track.items.push(TrackItem::Gap(Gap::new(frames(2))));
        track.items.push(TrackItem::Clip(clip(20)));
        let layout = TrackLayout::build(&track, RATE);

        assert_eq!(layout.len(), 2);
        assert!(!layout.is_empty());
        assert_eq!(layout.placements()[0].item_index, 1);
        assert_eq!(layout.placements()[0].range, range(5, 10));
        assert_eq!(layout.placements()[1].item_index, 3);
        assert_eq!(layout.placements()[1].range, range(17, 20));
        assert_eq!(layout.content_duration(RATE), frames(37));
        assert_eq!(TrackLayout::default().content_duration(RATE), frames(0));

        assert_eq!(layout.at(frames(5)).unwrap().item_index, 1);
        assert_eq!(layout.at(frames(14)).unwrap().item_index, 1);
        assert!(layout.at(frames(15)).is_none());
        assert!(layout.at(frames(100)).is_none());
    }

    #[test]
    fn visible_query_takes_clips_touching_either_edge_and_no_others() {
        // Ten clips of ten frames each, laid end to end.
        let layout = TrackLayout::build(&track_of(&[10; 10]), RATE);

        // A window inside clips 2..=4.
        let visible = layout.visible(range(25, 20));
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].range, range(20, 10));
        assert_eq!(visible[2].range, range(40, 10));

        // A window aligned exactly with one clip takes only that clip: the
        // neighbours touch it but share no instant with the window.
        let aligned = layout.visible(range(30, 10));
        assert_eq!(aligned.len(), 1);
        assert_eq!(aligned[0].range, range(30, 10));

        // Windows entirely outside the track's content.
        assert!(layout.visible(range(200, 50)).is_empty());
        assert!(layout.visible(TimeRange::empty_at(frames(35))).is_empty());
        // A window covering everything takes everything.
        assert_eq!(layout.visible(range(0, 1000)).len(), 10);
    }

    #[test]
    fn a_thousand_clips_only_lay_out_what_is_on_screen() {
        // A thousand one-second clips: nearly seven hours of timeline.
        let layout = TrackLayout::build(&track_of(&[24; 1000]), RATE);
        let mut view = view(zoom(2, 1), 1000);
        view.set_scroll_px(24 * 2 * 500);

        let visible = view.visible_clips(&layout);
        // 1000 pixels at two pixels per frame is 500 frames: 20 clips plus the
        // partly visible one at the trailing edge.
        assert_eq!(visible.len(), 21);
        assert_eq!(visible[0].range.start(), frames(500 * 24));
        assert!(visible.len() < layout.len() / 10);
        for placement in visible {
            assert!(placement.range.overlaps(view.visible_range()));
        }

        // Fitted to the whole sequence, every clip is visible.
        view.fit(layout.content_duration(RATE));
        assert_eq!(view.visible_clips(&layout).len(), 1000);
    }
}
