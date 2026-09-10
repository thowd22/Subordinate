//! The playback scheduler and its clock.
//!
//! Play/pause needs something that advances the playhead in real time and
//! tells the rest of the preview path which frame to show
//! (`Playhead -> Scheduler -> {decode requests} -> frame cache -> compositor`,
//! docs/PLAN.md §4). That is [`PlaybackScheduler`]: a clock that is driven by
//! wall-clock deltas handed to [`PlaybackScheduler::advance`] and answers with
//! the frame that should be on screen now.
//!
//! Three properties matter:
//!
//! - **No floats.** The clock accumulates elapsed nanoseconds in integer units
//!   scaled by the sequence timebase, so a frame boundary is crossed at
//!   exactly the same instant on every run and 1001/30000 rates never drift
//!   (docs/PLAN.md §5.1).
//! - **It drops rather than stalls.** The position follows the wall clock, not
//!   the last frame shown, so a tick that took longer than a frame interval —
//!   decode falling behind, a slow composite — skips the presentations that
//!   were missed, counts them and logs the running total. Playback stays in
//!   time; it never waits for a late frame.
//! - **It is not project state.** The playhead is where the user is looking,
//!   not part of the edit, so moving it is not a [`crate::Command`] and is not
//!   undoable. It is published as a [`PlayheadEvent`] instead.
//!
//! The audio clock replaces this one as the playback master in phase 3
//! (docs/PLAN.md §5.4); until then the video clock is the master and this is
//! it.
//!
//! ```
//! use std::time::Duration;
//!
//! use sub_edit::playback::{PlaybackScheduler, ShuttleSpeed};
//! use sub_time::{Rational, RationalTime};
//!
//! let mut scheduler = PlaybackScheduler::new(Rational::FPS_24);
//! scheduler.set_duration(RationalTime::new(48, Rational::FPS_24));
//!
//! // L plays forward, and repeats shuttle faster.
//! scheduler.play_forward();
//! assert_eq!(scheduler.speed(), ShuttleSpeed::Forward1x);
//! scheduler.play_forward();
//! assert_eq!(scheduler.speed(), ShuttleSpeed::Forward2x);
//!
//! // A tick one frame interval long advances two frames at 2x.
//! let tick = scheduler.advance(Duration::from_nanos(41_666_667)).unwrap();
//! assert_eq!(tick.position, RationalTime::new(2, Rational::FPS_24));
//! assert_eq!(tick.dropped, 0);
//!
//! // K pauses where it stands.
//! scheduler.pause();
//! assert!(!scheduler.is_playing());
//! ```

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sub_core::{SubError, SubResult};
use sub_model::Sequence;
use sub_time::{Rational, RationalTime, Rounding, TimeRange};

use crate::codes;

/// Nanoseconds in one second, as the clock's accumulator counts them.
const NANOS_PER_SECOND: i128 = 1_000_000_000;

/// How fast, and in which direction, playback is running.
///
/// JKL shuttling is the NLE convention: L plays forward and each further press
/// doubles the speed to 2x then 4x, J does the same backwards, and K pauses.
/// The set is closed at 4x, which is as fast as the MVP shuttles.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    JsonSchema,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ShuttleSpeed {
    /// Stopped: the playhead stays where it is.
    #[default]
    Paused,
    /// Forward at normal speed.
    Forward1x,
    /// Forward at twice normal speed.
    Forward2x,
    /// Forward at four times normal speed.
    Forward4x,
    /// Backwards at normal speed.
    Reverse1x,
    /// Backwards at twice normal speed.
    Reverse2x,
    /// Backwards at four times normal speed.
    Reverse4x,
}

impl ShuttleSpeed {
    /// The source frames covered per presented frame, negative when running
    /// backwards and zero when paused.
    #[must_use]
    pub const fn signed_multiplier(self) -> i64 {
        match self {
            Self::Paused => 0,
            Self::Forward1x => 1,
            Self::Forward2x => 2,
            Self::Forward4x => 4,
            Self::Reverse1x => -1,
            Self::Reverse2x => -2,
            Self::Reverse4x => -4,
        }
    }

    /// The speed without its direction: 0, 1, 2 or 4.
    #[must_use]
    pub const fn multiplier(self) -> i64 {
        self.signed_multiplier().abs()
    }

    /// Whether the playhead is standing still.
    #[must_use]
    pub const fn is_paused(self) -> bool {
        matches!(self, Self::Paused)
    }

    /// Whether playback is running backwards.
    #[must_use]
    pub const fn is_reverse(self) -> bool {
        self.signed_multiplier() < 0
    }

    /// The speed one press of L reaches from here: forward at 1x from paused
    /// or from any reverse speed, and the next step up when already forward.
    #[must_use]
    pub const fn faster_forward(self) -> Self {
        match self {
            Self::Forward1x => Self::Forward2x,
            Self::Forward2x | Self::Forward4x => Self::Forward4x,
            _ => Self::Forward1x,
        }
    }

    /// The speed one press of J reaches from here, the mirror of
    /// [`ShuttleSpeed::faster_forward`].
    #[must_use]
    pub const fn faster_reverse(self) -> Self {
        match self {
            Self::Reverse1x => Self::Reverse2x,
            Self::Reverse2x | Self::Reverse4x => Self::Reverse4x,
            _ => Self::Reverse1x,
        }
    }

    /// How the transport reads this speed, for a readout or a log line.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Paused => "paused",
            Self::Forward1x => "1x",
            Self::Forward2x => "2x",
            Self::Forward4x => "4x",
            Self::Reverse1x => "-1x",
            Self::Reverse2x => "-2x",
            Self::Reverse4x => "-4x",
        }
    }
}

/// What one presented frame of playback did.
///
/// A tick is produced only when the clock crossed a frame boundary: an
/// [`PlaybackScheduler::advance`] shorter than a frame interval returns
/// `None`, so a caller ticking faster than the timebase repaints nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tick {
    /// Where the playhead now is.
    pub position: RationalTime,
    /// How many frames it moved, negative when running backwards.
    pub frames_advanced: i64,
    /// How many presentations this tick missed because it took longer than a
    /// frame interval. Zero when playback is keeping up.
    pub dropped: u64,
    /// Whether the playhead wrapped around the loop range on this tick.
    pub wrapped: bool,
    /// Whether playback stopped on this tick, having reached an end of the
    /// sequence with no loop range in force.
    pub stopped: bool,
}

/// The playhead moved, as the engine broadcasts it.
///
/// It is a separate event from [`crate::ChangeEvent`] because nothing about it
/// is a project change: no revision, no undo, no command. Subscribers that
/// only care about edits never see these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct PlayheadEvent {
    /// Where the playhead is now.
    pub position: RationalTime,
    /// The speed it is running at, `paused` when it was moved by a seek.
    pub speed: ShuttleSpeed,
    /// Whether playback is running.
    pub playing: bool,
    /// Frames dropped so far in this run of playback; reset when playback
    /// starts again.
    pub dropped_frames: u64,
    /// Whether the playhead reached this position by wrapping the loop range.
    pub wrapped: bool,
}

/// The clock that drives playback.
///
/// It owns the playhead, the shuttle speed and the loop range, and nothing
/// else: decoding, compositing and audio all read from it. The caller drives
/// it, so it is deterministic and testable without a real timer — the engine
/// hands it the elapsed time between wake-ups, and a test hands it whatever
/// [`Duration`] it likes.
#[derive(Debug, Clone)]
pub struct PlaybackScheduler {
    /// The sequence timebase every position is expressed at.
    rate: Rational,
    /// Sequence length in frames; never negative.
    duration_frames: i64,
    /// The playhead, in frames from zero.
    frame: i64,
    /// How fast playback is running.
    speed: ShuttleSpeed,
    /// The loop range as `(start, end_exclusive)` frames, when one is set.
    loop_frames: Option<(i64, i64)>,
    /// Elapsed time not yet worth a frame, in nanoseconds scaled by the
    /// timebase numerator. Always in `0..NANOS_PER_SECOND * denominator`.
    residue: i128,
    /// Presentations missed in this run of playback.
    dropped: u64,
}

impl PlaybackScheduler {
    /// A scheduler for an empty sequence at `rate`, paused at frame zero.
    #[must_use]
    pub fn new(rate: Rational) -> Self {
        Self {
            rate,
            duration_frames: 0,
            frame: 0,
            speed: ShuttleSpeed::Paused,
            loop_frames: None,
            residue: 0,
            dropped: 0,
        }
    }

    /// A scheduler for `sequence`: its timebase, its length and frame zero.
    #[must_use]
    pub fn for_sequence(sequence: &Sequence) -> Self {
        let mut scheduler = Self::new(sequence.settings.frame_rate);
        scheduler.follow_sequence(sequence);
        scheduler
    }

    /// Re-reads the timebase and the length from `sequence`, keeping the
    /// playhead at the same instant.
    ///
    /// An edit that lengthens or shortens the sequence changes where playback
    /// stops, so whoever owns the scheduler calls this after the edit rather
    /// than letting the clock run past the end of what exists.
    pub fn follow_sequence(&mut self, sequence: &Sequence) {
        self.set_rate(sequence.settings.frame_rate);
        self.set_duration(sequence_duration(sequence));
    }

    /// The sequence timebase.
    #[must_use]
    pub const fn rate(&self) -> Rational {
        self.rate
    }

    /// Re-expresses the clock at a new timebase, rescaling the playhead, the
    /// duration and the loop range so all three keep pointing at the same
    /// instants.
    pub fn set_rate(&mut self, rate: Rational) {
        if rate == self.rate {
            return;
        }
        let position = self.position().rescaled_to(rate);
        let duration = self.duration().rescaled_to_rounding(rate, Rounding::Ceil);
        let loop_range = self.loop_frames.map(|(start, end)| {
            (
                RationalTime::new(start, self.rate)
                    .rescaled_to(rate)
                    .value(),
                RationalTime::new(end, self.rate)
                    .rescaled_to_rounding(rate, Rounding::Ceil)
                    .value(),
            )
        });
        self.rate = rate;
        self.duration_frames = duration.value().max(0);
        self.loop_frames = loop_range.filter(|(start, end)| end > start);
        self.residue = 0;
        self.seek(position);
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
    /// A duration that is not a whole number of frames is rounded up — a
    /// part-frame at the end is still a frame to show — and a negative one
    /// reads as empty.
    pub fn set_duration(&mut self, duration: RationalTime) {
        self.duration_frames = duration
            .rescaled_to_rounding(self.rate, Rounding::Ceil)
            .value()
            .max(0);
        self.frame = self.frame.clamp(0, self.last_frame_number());
    }

    /// The last frame that can be shown; zero for an empty sequence.
    #[must_use]
    pub const fn last_frame_number(&self) -> i64 {
        if self.duration_frames <= 1 {
            0
        } else {
            self.duration_frames - 1
        }
    }

    /// Where the playhead is.
    #[must_use]
    pub const fn position(&self) -> RationalTime {
        RationalTime::new(self.frame, self.rate)
    }

    /// Where the playhead is, as a frame number.
    #[must_use]
    pub const fn position_frames(&self) -> i64 {
        self.frame
    }

    /// Moves the playhead, clamped to the sequence, and resets the part-frame
    /// the clock had accumulated so the next frame is a whole one.
    ///
    /// Seeking does not stop playback: scrubbing while playing keeps playing
    /// from where it landed, which is what the scrub bar wants.
    pub fn seek(&mut self, position: RationalTime) {
        let frames = position
            .rescaled_to_rounding(self.rate, Rounding::Floor)
            .value();
        self.seek_frames(frames);
    }

    /// [`PlaybackScheduler::seek`] by frame number at the sequence timebase.
    pub fn seek_frames(&mut self, frame: i64) {
        self.frame = frame.clamp(0, self.last_frame_number());
        self.residue = 0;
    }

    /// The speed playback is running at.
    #[must_use]
    pub const fn speed(&self) -> ShuttleSpeed {
        self.speed
    }

    /// Whether playback is running.
    #[must_use]
    pub const fn is_playing(&self) -> bool {
        !self.speed.is_paused()
    }

    /// Sets the shuttle speed directly.
    ///
    /// Starting from paused resets the drop count, so the number reported
    /// belongs to this run of playback and not to the whole session.
    pub fn set_speed(&mut self, speed: ShuttleSpeed) {
        if speed == self.speed {
            return;
        }
        if !self.is_playing() && !speed.is_paused() {
            self.dropped = 0;
        }
        self.speed = speed;
        self.residue = 0;
    }

    /// L: play forward, and shuttle faster on every further press.
    pub fn play_forward(&mut self) {
        self.set_speed(self.speed.faster_forward());
    }

    /// J: play backwards, and shuttle faster on every further press.
    pub fn play_backward(&mut self) {
        self.set_speed(self.speed.faster_reverse());
    }

    /// K: stop where the playhead stands.
    pub fn pause(&mut self) {
        self.set_speed(ShuttleSpeed::Paused);
    }

    /// Space: play forward at 1x, or pause if anything is already playing.
    pub fn toggle(&mut self) {
        if self.is_playing() {
            self.pause();
        } else {
            self.set_speed(ShuttleSpeed::Forward1x);
        }
    }

    /// The loop range in force, if any.
    #[must_use]
    pub fn loop_range(&self) -> Option<TimeRange> {
        self.loop_frames.and_then(|(start, end)| {
            TimeRange::from_start_end(
                RationalTime::new(start, self.rate),
                RationalTime::new(end, self.rate),
            )
        })
    }

    /// Sets or clears the loop range: while one is in force, playback wraps
    /// around it instead of stopping at either end of the sequence.
    ///
    /// # Errors
    ///
    /// Returns `edit.invalid_time` when the range is empty or starts before
    /// zero: neither describes a span of frames that could be looped.
    pub fn set_loop_range(&mut self, range: Option<TimeRange>) -> SubResult<()> {
        let Some(range) = range else {
            self.loop_frames = None;
            return Ok(());
        };
        let start = range
            .start()
            .rescaled_to_rounding(self.rate, Rounding::Floor)
            .value();
        let end = range
            .end_exclusive()
            .rescaled_to_rounding(self.rate, Rounding::Ceil)
            .value();
        if start < 0 || end <= start {
            return Err(SubError::new(
                codes::INVALID_TIME,
                "a loop range must start at or after zero and cover at least one frame",
            )
            .with_detail("start", start.to_string())
            .with_detail("end", end.to_string()));
        }
        self.loop_frames = Some((start, end));
        Ok(())
    }

    /// Frames dropped so far in this run of playback.
    #[must_use]
    pub const fn dropped_frames(&self) -> u64 {
        self.dropped
    }

    /// One frame of the sequence, as wall-clock time.
    ///
    /// This is the presentation cadence: playback shows one composited frame
    /// per interval whatever the shuttle speed, and covers more source frames
    /// per interval as the speed goes up.
    #[must_use]
    pub fn frame_interval(&self) -> Duration {
        let nanos = NANOS_PER_SECOND * i128::from(self.rate.denominator())
            / i128::from(self.rate.numerator());
        Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }

    /// How long until the next frame is due, or `None` while paused.
    ///
    /// The engine thread waits exactly this long for a request before ticking
    /// again, so playback keeps time without polling.
    #[must_use]
    pub fn time_until_next_frame(&self) -> Option<Duration> {
        if !self.is_playing() {
            return None;
        }
        let numerator = i128::from(self.rate.numerator());
        let remaining = (self.interval_units() - self.residue).max(0);
        // Round up: waking a hair early would tick nothing and wait again.
        let nanos = (remaining + numerator - 1) / numerator;
        Some(Duration::from_nanos(
            u64::try_from(nanos).unwrap_or(u64::MAX),
        ))
    }

    /// Advances the clock by `elapsed` and returns the frame to present, if
    /// the clock crossed a frame boundary.
    ///
    /// Position follows the wall clock: when `elapsed` covers several frame
    /// intervals — decode fell behind, or the app was not scheduled — the
    /// presentations in between are counted as dropped and logged, and the
    /// playhead lands where the clock says it should rather than where the
    /// last frame left it.
    pub fn advance(&mut self, elapsed: Duration) -> Option<Tick> {
        if !self.is_playing() {
            return None;
        }
        let nanos = i128::try_from(elapsed.as_nanos()).unwrap_or(i128::MAX);
        self.residue += nanos * i128::from(self.rate.numerator());
        let interval = self.interval_units();
        let intervals = self.residue / interval;
        self.residue %= interval;
        let intervals = i64::try_from(intervals).unwrap_or(i64::MAX);
        if intervals == 0 {
            return None;
        }

        let dropped = u64::try_from(intervals - 1).unwrap_or(0);
        if dropped > 0 {
            self.dropped = self.dropped.saturating_add(dropped);
            tracing::warn!(
                dropped,
                total = self.dropped,
                speed = self.speed.label(),
                "playback fell behind; frames dropped to stay in time"
            );
        }

        let frames_advanced = intervals.saturating_mul(self.speed.signed_multiplier());
        let before = self.frame;
        let (frame, wrapped, stopped) = self.landing(before.saturating_add(frames_advanced));
        self.frame = frame;
        if stopped {
            self.speed = ShuttleSpeed::Paused;
            self.residue = 0;
        }
        Some(Tick {
            position: self.position(),
            frames_advanced: frame - before,
            dropped,
            wrapped,
            stopped,
        })
    }

    /// The event a subscriber is sent for the playhead as it stands, with
    /// `wrapped` describing how it got there.
    #[must_use]
    pub const fn event(&self, wrapped: bool) -> PlayheadEvent {
        PlayheadEvent {
            position: self.position(),
            speed: self.speed,
            playing: self.is_playing(),
            dropped_frames: self.dropped,
            wrapped,
        }
    }

    /// Where a target frame actually lands: wrapped into the loop range when
    /// one is in force, otherwise clamped to the sequence, stopping playback
    /// at whichever end it ran into.
    fn landing(&self, target: i64) -> (i64, bool, bool) {
        if let Some((start, end)) = self.loop_frames {
            let span = end - start;
            let last = end.min(self.duration_frames.max(1)) - 1;
            if target >= start && target <= last {
                return (target, false, false);
            }
            let wrapped = start + (target - start).rem_euclid(span);
            return (wrapped.clamp(0, self.last_frame_number()), true, false);
        }
        let last = self.last_frame_number();
        if target > last {
            return (last, false, true);
        }
        if target < 0 {
            return (0, false, true);
        }
        (target, false, false)
    }

    /// One frame interval in the accumulator's units.
    fn interval_units(&self) -> i128 {
        NANOS_PER_SECOND * i128::from(self.rate.denominator())
    }
}

/// How long `sequence` plays: the longest of its tracks.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame at 24 fps, to the nanosecond the clock counts in.
    const FRAME_24: Duration = Duration::from_nanos(41_666_667);

    fn scheduler(frames: i64) -> PlaybackScheduler {
        let mut scheduler = PlaybackScheduler::new(Rational::FPS_24);
        scheduler.set_duration(RationalTime::new(frames, Rational::FPS_24));
        scheduler
    }

    #[test]
    fn a_paused_clock_never_moves() {
        let mut scheduler = scheduler(48);
        assert!(!scheduler.is_playing());
        assert_eq!(scheduler.advance(Duration::from_secs(1)), None);
        assert_eq!(scheduler.position_frames(), 0);
        assert_eq!(scheduler.time_until_next_frame(), None);
    }

    #[test]
    fn a_tick_shorter_than_a_frame_presents_nothing() {
        let mut scheduler = scheduler(48);
        scheduler.play_forward();
        assert_eq!(scheduler.advance(Duration::from_millis(20)), None);
        assert_eq!(scheduler.position_frames(), 0);
        // The part-frame is kept, so two short ticks still make one frame.
        let tick = scheduler.advance(Duration::from_millis(22)).unwrap();
        assert_eq!(tick.frames_advanced, 1);
    }

    #[test]
    fn one_second_of_playback_is_exactly_the_frame_rate() {
        let mut scheduler = scheduler(200);
        scheduler.play_forward();
        // Ten ticks that add up to a second land on frame 24 exactly: the
        // accumulator keeps each part-frame rather than losing it per tick,
        // and 100ms is not a whole number of frames at 24 fps.
        for _ in 0..10 {
            scheduler.advance(Duration::from_millis(100));
        }
        assert_eq!(scheduler.position_frames(), 24);
    }

    #[test]
    fn a_fractional_rate_does_not_drift() {
        let mut scheduler = PlaybackScheduler::new(Rational::FPS_23_976);
        scheduler.set_duration(RationalTime::new(10_000, Rational::FPS_23_976));
        scheduler.play_forward();
        // Ten seconds of wall clock at 24000/1001 fps is 239 whole frames.
        for _ in 0..10 {
            scheduler.advance(Duration::from_secs(1));
        }
        assert_eq!(scheduler.position_frames(), 239);
    }

    #[test]
    fn jkl_shuttles_through_the_speeds() {
        let mut scheduler = scheduler(48);
        scheduler.play_forward();
        assert_eq!(scheduler.speed(), ShuttleSpeed::Forward1x);
        scheduler.play_forward();
        assert_eq!(scheduler.speed(), ShuttleSpeed::Forward2x);
        scheduler.play_forward();
        assert_eq!(scheduler.speed(), ShuttleSpeed::Forward4x);
        scheduler.play_forward();
        assert_eq!(scheduler.speed(), ShuttleSpeed::Forward4x);

        // J from forward turns round at 1x rather than jumping to 4x reverse.
        scheduler.play_backward();
        assert_eq!(scheduler.speed(), ShuttleSpeed::Reverse1x);
        scheduler.play_backward();
        assert_eq!(scheduler.speed(), ShuttleSpeed::Reverse2x);
        scheduler.play_backward();
        assert_eq!(scheduler.speed(), ShuttleSpeed::Reverse4x);

        scheduler.pause();
        assert_eq!(scheduler.speed(), ShuttleSpeed::Paused);
        scheduler.toggle();
        assert_eq!(scheduler.speed(), ShuttleSpeed::Forward1x);
        scheduler.toggle();
        assert_eq!(scheduler.speed(), ShuttleSpeed::Paused);
    }

    #[test]
    fn shuttle_speed_scales_the_frames_covered_per_tick() {
        for (speed, expected) in [
            (ShuttleSpeed::Forward1x, 1),
            (ShuttleSpeed::Forward2x, 2),
            (ShuttleSpeed::Forward4x, 4),
        ] {
            let mut scheduler = scheduler(48);
            scheduler.set_speed(speed);
            let tick = scheduler.advance(FRAME_24).unwrap();
            assert_eq!(tick.frames_advanced, expected, "{speed:?}");
            assert_eq!(tick.dropped, 0);
        }
    }

    #[test]
    fn reverse_playback_runs_backwards_and_stops_at_zero() {
        let mut scheduler = scheduler(48);
        scheduler.seek_frames(6);
        scheduler.set_speed(ShuttleSpeed::Reverse2x);
        let tick = scheduler.advance(FRAME_24).unwrap();
        assert_eq!(tick.frames_advanced, -2);
        assert_eq!(scheduler.position_frames(), 4);

        let tick = scheduler.advance(FRAME_24 * 4).unwrap();
        assert_eq!(scheduler.position_frames(), 0);
        assert!(tick.stopped);
        assert!(!scheduler.is_playing());
    }

    #[test]
    fn playback_stops_at_the_last_frame_without_a_loop() {
        let mut scheduler = scheduler(10);
        scheduler.play_forward();
        let tick = scheduler.advance(FRAME_24 * 100).unwrap();
        assert_eq!(scheduler.position_frames(), 9);
        assert!(tick.stopped);
        assert!(!tick.wrapped);
        assert!(!scheduler.is_playing());
    }

    #[test]
    fn a_loop_range_wraps_instead_of_stopping() {
        let mut scheduler = scheduler(48);
        let range = TimeRange::from_start_end(
            RationalTime::new(4, Rational::FPS_24),
            RationalTime::new(8, Rational::FPS_24),
        )
        .unwrap();
        scheduler.set_loop_range(Some(range)).unwrap();
        assert_eq!(scheduler.loop_range(), Some(range));

        scheduler.seek_frames(7);
        scheduler.play_forward();
        let tick = scheduler.advance(FRAME_24).unwrap();
        assert_eq!(scheduler.position_frames(), 4);
        assert!(tick.wrapped);
        assert!(!tick.stopped);
        assert!(scheduler.is_playing());

        // Backwards it wraps the other way, round to the last looped frame.
        scheduler.set_speed(ShuttleSpeed::Reverse1x);
        scheduler.advance(FRAME_24).unwrap();
        assert_eq!(scheduler.position_frames(), 7);
    }

    #[test]
    fn an_empty_loop_range_is_refused() {
        let mut scheduler = scheduler(48);
        let empty = TimeRange::empty_at(RationalTime::new(4, Rational::FPS_24));
        let err = scheduler.set_loop_range(Some(empty)).unwrap_err();
        assert_eq!(err.code, codes::INVALID_TIME);
        assert_eq!(scheduler.loop_range(), None);
        scheduler.set_loop_range(None).unwrap();
        assert_eq!(scheduler.loop_range(), None);
    }

    #[test]
    fn a_late_tick_drops_frames_rather_than_falling_behind() {
        let mut scheduler = scheduler(200);
        scheduler.play_forward();
        // One tick that took five frame intervals: four presentations missed.
        let tick = scheduler.advance(FRAME_24 * 5).unwrap();
        assert_eq!(tick.frames_advanced, 5);
        assert_eq!(tick.dropped, 4);
        assert_eq!(scheduler.dropped_frames(), 4);
        assert_eq!(scheduler.position_frames(), 5);

        // Catching up adds nothing to the count.
        let tick = scheduler.advance(FRAME_24).unwrap();
        assert_eq!(tick.dropped, 0);
        assert_eq!(scheduler.dropped_frames(), 4);
    }

    #[test]
    fn the_drop_count_belongs_to_one_run_of_playback() {
        let mut scheduler = scheduler(200);
        scheduler.play_forward();
        scheduler.advance(FRAME_24 * 3);
        assert_eq!(scheduler.dropped_frames(), 2);
        scheduler.pause();
        assert_eq!(scheduler.dropped_frames(), 2);
        scheduler.play_forward();
        assert_eq!(scheduler.dropped_frames(), 0);
    }

    #[test]
    fn seeking_keeps_playing_and_starts_the_next_frame_whole() {
        let mut scheduler = scheduler(48);
        scheduler.play_forward();
        scheduler.advance(Duration::from_millis(30));
        scheduler.seek(RationalTime::new(20, Rational::FPS_24));
        assert_eq!(scheduler.position_frames(), 20);
        assert!(scheduler.is_playing());
        // The part-frame was dropped by the seek, so a short tick shows nothing.
        assert_eq!(scheduler.advance(Duration::from_millis(30)), None);
    }

    #[test]
    fn seeking_is_clamped_to_the_sequence() {
        let mut scheduler = scheduler(10);
        scheduler.seek(RationalTime::new(100, Rational::FPS_24));
        assert_eq!(scheduler.position_frames(), 9);
        scheduler.seek(RationalTime::new(-5, Rational::FPS_24));
        assert_eq!(scheduler.position_frames(), 0);
    }

    #[test]
    fn the_wait_until_the_next_frame_shrinks_as_the_clock_runs() {
        let mut scheduler = scheduler(48);
        scheduler.play_forward();
        let full = scheduler.time_until_next_frame().unwrap();
        assert!(full >= scheduler.frame_interval(), "{full:?}");
        assert!(
            full <= scheduler.frame_interval() + Duration::from_nanos(1),
            "{full:?}"
        );
        scheduler.advance(Duration::from_millis(20));
        let remaining = scheduler.time_until_next_frame().unwrap();
        assert!(remaining < Duration::from_millis(22), "{remaining:?}");
        assert!(remaining > Duration::from_millis(21), "{remaining:?}");
    }

    #[test]
    fn changing_the_timebase_keeps_the_same_instant() {
        let mut scheduler = scheduler(48);
        scheduler.seek_frames(24);
        let range = TimeRange::from_start_end(
            RationalTime::new(0, Rational::FPS_24),
            RationalTime::new(24, Rational::FPS_24),
        )
        .unwrap();
        scheduler.set_loop_range(Some(range)).unwrap();
        let fps_48 = Rational::new(48, 1).unwrap();
        scheduler.set_rate(fps_48);
        assert_eq!(scheduler.rate(), fps_48);
        assert_eq!(scheduler.position_frames(), 48);
        assert_eq!(scheduler.duration_frames(), 96);
        assert_eq!(scheduler.loop_range(), Some(range));
    }

    #[test]
    fn the_event_describes_the_playhead() {
        let mut scheduler = scheduler(48);
        scheduler.play_forward();
        scheduler.advance(FRAME_24 * 3);
        let event = scheduler.event(false);
        assert_eq!(event.position, RationalTime::new(3, Rational::FPS_24));
        assert_eq!(event.speed, ShuttleSpeed::Forward1x);
        assert!(event.playing);
        assert_eq!(event.dropped_frames, 2);
        assert!(!event.wrapped);
    }

    #[test]
    fn an_empty_sequence_has_nowhere_to_go() {
        let mut scheduler = PlaybackScheduler::new(Rational::FPS_24);
        scheduler.play_forward();
        let tick = scheduler.advance(FRAME_24).unwrap();
        assert_eq!(scheduler.position_frames(), 0);
        assert!(tick.stopped);
    }
}
