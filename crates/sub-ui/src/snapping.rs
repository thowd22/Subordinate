//! Snapping: pulling a time onto the edges an editor actually cares about.
//!
//! Precise placement on a timeline is not done by aiming a mouse at a
//! subpixel. It is done by getting close to something meaningful — the end of
//! the clip before, a marker, the playhead, the head of the sequence — and
//! letting the editor land exactly on it. That is what this module is: a set
//! of [`SnapCandidate`]s gathered from what is on screen, and a nearest-match
//! search over them.
//!
//! Two properties matter and are both tested here.
//!
//! **The threshold is in pixels.** "Close enough" is a property of the hand
//! and the screen, not of the sequence, so it is measured in points and stays
//! the same however far in or out the timeline is zoomed. Zoomed out, a few
//! pixels are seconds of programme; zoomed in, they are a fraction of a
//! frame. That is the intent: what the user can point at is what snaps.
//!
//! **The arithmetic is exact.** Distances are never converted to seconds or
//! to floats. A distance in frames is compared against the threshold by
//! cross-multiplying with the zoom's pixels-per-frame fraction, in `i128`, so
//! the answer is the same at every zoom and never drifts (docs/PLAN.md:
//! timeline math is [`RationalTime`], never floats).
//!
//! The module knows nothing about painting or about egui. The timeline panel
//! collects candidates with [`collect_candidates`] and resolves a pointer
//! position with [`snap`]; a later drag or trim tool uses the same two calls.

use sub_model::{Sequence, TrackItem};
use sub_time::{Rational, RationalTime, TimeRange};

use crate::timeline::{TimelineView, TrackLayout};

/// The default snap threshold, in points.
///
/// Wide enough that an ordinary hand lands inside it, narrow enough that two
/// cuts a few frames apart at a close zoom stay separately reachable.
pub const DEFAULT_THRESHOLD_PX: u32 = 8;

/// What a snap candidate is, which is also how ties between two candidates at
/// the same distance are broken.
///
/// The order is the priority order, strongest first: an editor aiming between
/// a marker and a clip edge that sit on the same frame means the marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SnapKind {
    /// Where the playhead is now.
    Playhead,
    /// A marker on the sequence.
    Marker,
    /// The start or the end of a clip.
    ClipEdge,
    /// Sequence time zero.
    SequenceStart,
}

impl SnapKind {
    /// Every kind, in priority order.
    pub const ALL: [Self; 4] = [
        Self::Playhead,
        Self::Marker,
        Self::ClipEdge,
        Self::SequenceStart,
    ];

    /// A stable identifier, for logging and for tests.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Playhead => "playhead",
            Self::Marker => "marker",
            Self::ClipEdge => "clip_edge",
            Self::SequenceStart => "sequence_start",
        }
    }
}

/// One instant something could snap to, and what put it there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapCandidate {
    /// The instant, in sequence time.
    pub time: RationalTime,
    /// What the instant is.
    pub kind: SnapKind,
}

impl SnapCandidate {
    /// A candidate of `kind` at `time`.
    #[must_use]
    pub const fn new(kind: SnapKind, time: RationalTime) -> Self {
        Self { time, kind }
    }
}

/// Whether snapping is on, and how close counts as close.
///
/// This is view state, not project state: it is a property of how the editor
/// is being driven, so it is never a command and never reaches the project
/// file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapSettings {
    /// Whether a time is pulled onto candidates at all.
    pub enabled: bool,
    /// How far from a candidate a time still snaps to it, in points.
    pub threshold_px: u32,
}

impl Default for SnapSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_px: DEFAULT_THRESHOLD_PX,
        }
    }
}

impl SnapSettings {
    /// Snapping on, at the default threshold.
    #[must_use]
    pub const fn on() -> Self {
        Self {
            enabled: true,
            threshold_px: DEFAULT_THRESHOLD_PX,
        }
    }

    /// Snapping off, at the default threshold.
    ///
    /// The threshold is kept so that turning snapping back on restores the
    /// feel the user had before.
    #[must_use]
    pub const fn off() -> Self {
        Self {
            enabled: false,
            threshold_px: DEFAULT_THRESHOLD_PX,
        }
    }

    /// Flips snapping on or off, and reports the new state.
    ///
    /// This is what the `S` shortcut
    /// ([`Action::ToggleSnapping`](crate::shortcuts::Action::ToggleSnapping))
    /// calls.
    pub const fn toggle(&mut self) -> bool {
        self.enabled = !self.enabled;
        self.enabled
    }
}

/// Gathers everything inside `range` that a time may snap to.
///
/// The candidates are the ones docs/PLAN.md §5.7 calls for: clip edges,
/// markers, the playhead and the start of the sequence. `layouts` are the
/// panel's per-track indexes, in track order, so the clip edges cost a binary
/// search per track rather than a walk of the whole sequence: only what the
/// viewport touches is ever considered.
///
/// `range` is normally the panel's visible range. Anything outside it is more
/// than a viewport away from the pointer and so can never be within a
/// threshold of it.
///
/// Candidates are appended to `into`, which is cleared first, and come out
/// sorted by time with duplicates of the same time and kind removed. The
/// buffer is the caller's so that a scrub, which does this every frame,
/// allocates nothing after its first.
pub fn collect_candidates(
    sequence: &Sequence,
    layouts: &[TrackLayout],
    playhead: Option<RationalTime>,
    range: TimeRange,
    into: &mut Vec<SnapCandidate>,
) {
    into.clear();
    let rate = sequence.settings.frame_rate;
    push_in_range(
        into,
        SnapKind::SequenceStart,
        RationalTime::zero(rate),
        range,
    );
    for layout in layouts {
        for placement in layout.visible(range) {
            into.push(SnapCandidate::new(
                SnapKind::ClipEdge,
                placement.range.start(),
            ));
            into.push(SnapCandidate::new(
                SnapKind::ClipEdge,
                placement.range.end_exclusive(),
            ));
        }
    }
    for marker in &sequence.markers {
        push_in_range(into, SnapKind::Marker, marker.marked_range.start(), range);
        if !marker.marked_range.is_empty() {
            push_in_range(
                into,
                SnapKind::Marker,
                marker.marked_range.end_exclusive(),
                range,
            );
        }
    }
    if let Some(playhead) = playhead {
        push_in_range(into, SnapKind::Playhead, playhead, range);
    }
    into.sort_unstable_by(|left, right| {
        left.time
            .cmp(&right.time)
            .then_with(|| left.kind.cmp(&right.kind))
    });
    into.dedup();
}

/// The edges of the clips of `track`, whether or not they are on screen.
///
/// A track that has no [`TrackLayout`] built for it yet — a freshly loaded
/// sequence, a test — can still offer its edges through this, at the cost of
/// a walk of the whole track. The panel uses the indexed path above.
pub fn track_edges(track: &sub_model::Track, rate: Rational, into: &mut Vec<SnapCandidate>) {
    for (item, range) in track.placements(rate) {
        if matches!(item, TrackItem::Clip(_)) {
            into.push(SnapCandidate::new(SnapKind::ClipEdge, range.start()));
            into.push(SnapCandidate::new(
                SnapKind::ClipEdge,
                range.end_exclusive(),
            ));
        }
    }
}

/// Pushes a candidate only when it falls inside `range`, ends included.
fn push_in_range(
    into: &mut Vec<SnapCandidate>,
    kind: SnapKind,
    time: RationalTime,
    range: TimeRange,
) {
    if time >= range.start() && time <= range.end_exclusive() {
        into.push(SnapCandidate::new(kind, time));
    }
}

/// The candidate `time` should snap to, if any is close enough.
///
/// Returns `None` when snapping is off, when nothing is within
/// [`SnapSettings::threshold_px`] points of `time` at the view's current
/// zoom, or when `candidates` is empty. Ties at equal distance are broken by
/// [`SnapKind`] priority and then by the earlier time, so the answer is the
/// same however the candidates were ordered.
///
/// The threshold is a screen distance: at 1 pixel per frame it is eight
/// frames, at 64 pixels per frame it is an eighth of a frame — which rounds
/// to the frame itself, so a close zoom snaps only to what the pointer is
/// practically on top of.
#[must_use]
pub fn snap(
    view: &TimelineView,
    time: RationalTime,
    candidates: &[SnapCandidate],
    settings: SnapSettings,
) -> Option<SnapCandidate> {
    if !settings.enabled {
        return None;
    }
    let mut best: Option<(i128, SnapCandidate)> = None;
    for &candidate in candidates {
        let Some(distance) = pixel_distance_scaled(view, time, candidate.time) else {
            continue;
        };
        if distance > threshold_scaled(view, settings.threshold_px) {
            continue;
        }
        let better = match best {
            None => true,
            Some((best_distance, best_candidate)) => {
                (distance, candidate.kind, candidate.time)
                    < (best_distance, best_candidate.kind, best_candidate.time)
            }
        };
        if better {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

/// `time`, snapped if anything is close enough and left alone otherwise.
///
/// The convenience form of [`snap`] for callers that only want the instant.
#[must_use]
pub fn snapped_time(
    view: &TimelineView,
    time: RationalTime,
    candidates: &[SnapCandidate],
    settings: SnapSettings,
) -> RationalTime {
    snap(view, time, candidates, settings).map_or(time, |candidate| candidate.time)
}

/// The distance between two instants in pixels, multiplied up by the zoom's
/// denominator so it stays an integer.
///
/// Compare it against [`threshold_scaled`] of the same view; the common
/// factor cancels. `None` when the two times cannot be brought to a common
/// timebase without overflowing, which no real sequence does.
fn pixel_distance_scaled(
    view: &TimelineView,
    left: RationalTime,
    right: RationalTime,
) -> Option<i128> {
    let rate = view.rate();
    let left = left.checked_rescaled_to(rate)?;
    let right = right.checked_rescaled_to(rate)?;
    let frames = i128::from(left.value()) - i128::from(right.value());
    Some(frames.abs() * i128::from(view.zoom().pixels_per_frame().numerator()))
}

/// The threshold in the same scaled units as [`pixel_distance_scaled`].
fn threshold_scaled(view: &TimelineView, threshold_px: u32) -> i128 {
    i128::from(threshold_px) * i128::from(view.zoom().pixels_per_frame().denominator())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sub_model::marker::Marker;
    use sub_model::sequence::SequenceSettings;
    use sub_model::{Clip, MediaId, Track, TrackKind};

    use crate::timeline::ZoomLevel;

    const RATE: Rational = Rational::FPS_24;

    fn frames(value: i64) -> RationalTime {
        RationalTime::new(value, RATE)
    }

    fn range(start: i64, duration: i64) -> TimeRange {
        TimeRange::new(frames(start), frames(duration)).expect("a valid range")
    }

    /// A sequence with clips at 0..48 and 96..144 on one track.
    fn scene() -> Sequence {
        let mut sequence = Sequence::new("edit", SequenceSettings::default());
        let mut track = Track::new("V1", TrackKind::Video);
        track.items.push(sub_model::TrackItem::Clip(Clip::new(
            "a",
            MediaId::new(),
            range(0, 48),
        )));
        track
            .items
            .push(sub_model::TrackItem::Gap(sub_model::track::Gap::new(
                frames(48),
            )));
        track.items.push(sub_model::TrackItem::Clip(Clip::new(
            "b",
            MediaId::new(),
            range(0, 48),
        )));
        sequence.tracks.push(track);
        sequence
    }

    fn layouts(sequence: &Sequence) -> Vec<TrackLayout> {
        sequence
            .tracks
            .iter()
            .map(|track| TrackLayout::build(track, sequence.settings.frame_rate))
            .collect()
    }

    fn view(zoom: ZoomLevel) -> TimelineView {
        let mut view = TimelineView::new(RATE);
        view.set_width_px(1000);
        view.set_zoom(zoom);
        view
    }

    fn times_of(candidates: &[SnapCandidate], kind: SnapKind) -> Vec<i64> {
        candidates
            .iter()
            .filter(|candidate| candidate.kind == kind)
            .map(|candidate| candidate.time.value())
            .collect()
    }

    #[test]
    fn candidates_are_the_clip_edges_markers_playhead_and_sequence_start() {
        let mut sequence = scene();
        sequence
            .markers
            .push(Marker::new("cue", TimeRange::empty_at(frames(60))));
        let layouts = layouts(&sequence);
        let mut candidates = Vec::new();
        collect_candidates(
            &sequence,
            &layouts,
            Some(frames(30)),
            range(0, 200),
            &mut candidates,
        );

        assert_eq!(times_of(&candidates, SnapKind::SequenceStart), vec![0]);
        assert_eq!(
            times_of(&candidates, SnapKind::ClipEdge),
            vec![0, 48, 96, 144]
        );
        assert_eq!(times_of(&candidates, SnapKind::Marker), vec![60]);
        assert_eq!(times_of(&candidates, SnapKind::Playhead), vec![30]);
        assert!(
            candidates
                .windows(2)
                .all(|pair| pair[0].time <= pair[1].time),
            "candidates come out sorted by time"
        );
    }

    #[test]
    fn a_span_marker_offers_both_of_its_ends() {
        let mut sequence = scene();
        sequence.markers.push(Marker::new("section", range(72, 24)));
        let layouts = layouts(&sequence);
        let mut candidates = Vec::new();
        collect_candidates(&sequence, &layouts, None, range(0, 200), &mut candidates);
        assert_eq!(times_of(&candidates, SnapKind::Marker), vec![72, 96]);
    }

    #[test]
    fn only_what_the_viewport_touches_is_collected() {
        let sequence = scene();
        let layouts = layouts(&sequence);
        let mut candidates = Vec::new();
        collect_candidates(
            &sequence,
            &layouts,
            Some(frames(500)),
            range(100, 60),
            &mut candidates,
        );
        assert_eq!(
            times_of(&candidates, SnapKind::ClipEdge),
            vec![96, 144],
            "the second clip's edges only"
        );
        assert!(
            times_of(&candidates, SnapKind::SequenceStart).is_empty(),
            "time zero is off screen"
        );
        assert!(
            times_of(&candidates, SnapKind::Playhead).is_empty(),
            "the playhead is off screen"
        );
    }

    #[test]
    fn the_nearest_candidate_inside_the_threshold_wins() {
        let sequence = scene();
        let layouts = layouts(&sequence);
        let mut candidates = Vec::new();
        collect_candidates(&sequence, &layouts, None, range(0, 200), &mut candidates);
        let view = view(ZoomLevel::ONE);

        // Five frames is five pixels here, inside the eight-pixel threshold.
        let snapped = snap(&view, frames(53), &candidates, SnapSettings::on())
            .expect("the cut at 48 is close enough");
        assert_eq!(snapped.time, frames(48));
        assert_eq!(snapped.kind, SnapKind::ClipEdge);

        // Ten frames is ten pixels, outside it.
        assert_eq!(
            snap(&view, frames(58), &candidates, SnapSettings::on()),
            None
        );
    }

    #[test]
    fn snapping_off_never_moves_a_time() {
        let sequence = scene();
        let layouts = layouts(&sequence);
        let mut candidates = Vec::new();
        collect_candidates(&sequence, &layouts, None, range(0, 200), &mut candidates);
        let view = view(ZoomLevel::ONE);
        assert_eq!(
            snap(&view, frames(49), &candidates, SnapSettings::off()),
            None
        );
        assert_eq!(
            snapped_time(&view, frames(49), &candidates, SnapSettings::off()),
            frames(49)
        );
    }

    #[test]
    fn the_threshold_is_pixels_so_it_narrows_in_time_as_the_zoom_climbs() {
        let sequence = scene();
        let layouts = layouts(&sequence);
        let mut candidates = Vec::new();
        collect_candidates(&sequence, &layouts, None, range(0, 200), &mut candidates);
        let settings = SnapSettings::on();

        // Eight pixels is eight frames at 1 px/frame, two frames at 4, and
        // less than a frame at 16 — the same reach for the hand at every
        // zoom, and a different reach in time.
        let reach = |pixels_per_frame: u32| -> i64 {
            let zoom =
                ZoomLevel::clamped(Rational::new(pixels_per_frame, 1).expect("a positive zoom"));
            let view = view(zoom);
            (0..=64)
                .take_while(|offset| {
                    snap(&view, frames(48 + offset), &candidates, settings)
                        .is_some_and(|found| found.time == frames(48))
                })
                .count()
                .try_into()
                .unwrap_or(i64::MAX)
        };
        assert_eq!(
            reach(1),
            9,
            "offsets 0..=8 frames snap at one pixel a frame"
        );
        assert_eq!(reach(4), 3, "offsets 0..=2 frames snap at four");
        assert_eq!(reach(16), 1, "only the frame itself snaps at sixteen");
    }

    #[test]
    fn zooming_does_not_change_which_of_two_candidates_is_nearer() {
        let sequence = scene();
        let layouts = layouts(&sequence);
        let mut candidates = Vec::new();
        collect_candidates(
            &sequence,
            &layouts,
            Some(frames(52)),
            range(0, 200),
            &mut candidates,
        );
        // 50 is nearer the cut at 48 than the playhead at 52 by one frame; a
        // pixel scale cannot change that, only whether either is in reach.
        for pixels_per_frame in [1_u32, 2, 4] {
            let zoom =
                ZoomLevel::clamped(Rational::new(pixels_per_frame, 1).expect("a positive zoom"));
            let view = view(zoom);
            let found = snap(&view, frames(49), &candidates, SnapSettings::on())
                .expect("something is in reach");
            assert_eq!(
                found.time,
                frames(48),
                "at {pixels_per_frame} pixels per frame"
            );
        }
    }

    #[test]
    fn a_tie_goes_to_the_stronger_kind() {
        let rate = RATE;
        let mut view = TimelineView::new(rate);
        view.set_width_px(1000);
        let candidates = [
            SnapCandidate::new(SnapKind::ClipEdge, frames(50)),
            SnapCandidate::new(SnapKind::Playhead, frames(50)),
            SnapCandidate::new(SnapKind::Marker, frames(50)),
        ];
        let found = snap(&view, frames(50), &candidates, SnapSettings::on()).expect("an exact hit");
        assert_eq!(found.kind, SnapKind::Playhead);
    }

    #[test]
    fn toggling_flips_snapping_and_keeps_the_threshold() {
        let mut settings = SnapSettings::on();
        assert!(!settings.toggle());
        assert!(!settings.enabled);
        assert_eq!(settings.threshold_px, DEFAULT_THRESHOLD_PX);
        assert!(settings.toggle());
        assert!(settings.enabled);
    }

    #[test]
    fn every_kind_has_a_distinct_stable_id() {
        let mut ids: Vec<&str> = SnapKind::ALL.iter().map(|kind| kind.id()).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "ids are distinct");
    }

    #[test]
    fn track_edges_reads_a_track_with_no_index_built() {
        let sequence = scene();
        let mut candidates = Vec::new();
        track_edges(&sequence.tracks[0], RATE, &mut candidates);
        assert_eq!(
            times_of(&candidates, SnapKind::ClipEdge),
            vec![0, 48, 96, 144]
        );
    }
}
