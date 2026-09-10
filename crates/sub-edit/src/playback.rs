//! The playback scheduler, and the clock the video follows.
//!
//! Play/pause needs something that tells the rest of the preview path which
//! frame to show
//! (`Playhead -> Scheduler -> {decode requests} -> frame cache -> compositor`,
//! docs/PLAN.md §4). That is [`PlaybackScheduler`].
//!
//! **Video has no clock of its own.** Sync is defined by audio: the master
//! reports a media time — for real playback the audio callback's position, as
//! `sub_audio::clock::AudioClock` publishes it — and
//! [`PlaybackScheduler::follow`] picks the frame that covers it
//! (docs/PLAN.md §5.4). When nothing is playing out of an audio device there
//! is still a master, [`MonotonicClock`], which turns elapsed wall time into
//! the same kind of media time;
//! [`PlaybackScheduler::advance`] is that master and [`PlaybackScheduler::follow`]
//! in one call. Either way the scheduler only ever chooses a frame for a time
//! somebody else measured.
//!
//! Three properties matter:
//!
//! - **No floats.** The fallback master accumulates elapsed nanoseconds in
//!   integer units scaled by the sequence timebase, and a master time is
//!   converted to a frame by exact rational rescaling, so a frame boundary is
//!   crossed at exactly the same instant on every run and 1001/30000 rates
//!   never drift (docs/PLAN.md §5.1).
//! - **It drops rather than stalls.** The frame is chosen for the master's
//!   time, not for the last frame shown, so when a tick covers several frame
//!   intervals — decode falling behind, a slow composite — the presentations
//!   in between are counted as dropped and the playhead lands where the master
//!   says. Playback stays in time; it never waits for a late frame.
//! - **It is not project state.** The playhead is where the user is looking,
//!   not part of the edit, so moving it is not a [`crate::Command`] and is not
//!   undoable. It is published as a [`PlayheadEvent`] instead.
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
/// A tick is produced only when the master crossed a frame boundary: an
/// [`PlaybackScheduler::advance`] shorter than a frame interval, or a
/// [`PlaybackScheduler::follow`] of a master time still inside the frame on
/// screen, returns `None`, so a caller ticking faster than the timebase
/// repaints nothing.
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

/// The master clock used when no audio stream is playing.
///
/// It measures the same thing an audio clock does — how much media time has
/// gone by — but from the wall clock instead of from rendered samples, so a
/// silent sequence, a shuttle speed no device is playing at, and a headless
/// engine all still have a master to follow.
///
/// It counts in integers: elapsed nanoseconds scaled by the timebase
/// numerator, with the part-frame carried between calls, so a fractional rate
/// never drifts.
///
/// ```
/// use std::time::Duration;
///
/// use sub_edit::playback::{MonotonicClock, ShuttleSpeed};
/// use sub_time::{Rational, RationalTime};
///
/// let mut clock = MonotonicClock::new(Rational::FPS_24);
/// clock.reset_to(10);
/// let position = clock.advance(Duration::from_millis(500), ShuttleSpeed::Forward1x);
/// assert_eq!(position, RationalTime::new(22, Rational::FPS_24));
/// ```
#[derive(Debug, Clone)]
pub struct MonotonicClock {
    /// The timebase the position is counted at.
    rate: Rational,
    /// Media time so far, in frames at `rate`. Signed: reverse shuttling runs
    /// it backwards, and the scheduler clamps what it does with it.
    frame: i64,
    /// Elapsed time not yet worth a frame, in nanoseconds scaled by the
    /// timebase numerator. Always in `0..NANOS_PER_SECOND * denominator`.
    residue: i128,
}

impl MonotonicClock {
    /// A clock at `rate`, reading zero.
    #[must_use]
    pub const fn new(rate: Rational) -> Self {
        Self {
            rate,
            frame: 0,
            residue: 0,
        }
    }

    /// The timebase the position is counted at.
    #[must_use]
    pub const fn rate(&self) -> Rational {
        self.rate
    }

    /// Re-expresses the clock at `rate`, keeping the same instant and
    /// dropping the part-frame so the next frame is a whole one.
    pub fn set_rate(&mut self, rate: Rational) {
        if rate == self.rate {
            return;
        }
        self.frame = self.position().rescaled_to(rate).value();
        self.rate = rate;
        self.residue = 0;
    }

    /// Moves the clock to `frame` and drops the part-frame it had
    /// accumulated, so that the next frame boundary is a whole interval away.
    ///
    /// A seek calls this so that the fallback master reads the same instant
    /// the playhead was put at.
    pub const fn reset_to(&mut self, frame: i64) {
        self.frame = frame;
        self.residue = 0;
    }

    /// The media time the clock has reached.
    #[must_use]
    pub const fn position(&self) -> RationalTime {
        RationalTime::new(self.frame, self.rate)
    }

    /// Adds `elapsed` of wall time at `speed` and returns the media time
    /// reached, which is unchanged when the part-frame is not yet whole.
    pub fn advance(&mut self, elapsed: Duration, speed: ShuttleSpeed) -> RationalTime {
        if speed.is_paused() {
            return self.position();
        }
        let nanos = i128::try_from(elapsed.as_nanos()).unwrap_or(i128::MAX);
        self.residue += nanos * i128::from(self.rate.numerator());
        let interval = self.interval_units();
        let intervals = i64::try_from(self.residue / interval).unwrap_or(i64::MAX);
        self.residue %= interval;
        self.frame = self
            .frame
            .saturating_add(intervals.saturating_mul(speed.signed_multiplier()));
        self.position()
    }

    /// How long until the clock crosses its next frame boundary.
    #[must_use]
    pub fn time_until_next_frame(&self) -> Duration {
        let numerator = i128::from(self.rate.numerator());
        let remaining = (self.interval_units() - self.residue).max(0);
        // Round up: waking a hair early would tick nothing and wait again.
        let nanos = (remaining + numerator - 1) / numerator;
        Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }

    /// One frame interval in the accumulator's units.
    fn interval_units(&self) -> i128 {
        NANOS_PER_SECOND * i128::from(self.rate.denominator())
    }
}

/// The playhead, and the frame chosen for whatever the master clock reads.
///
/// It owns the playhead, the shuttle speed and the loop range, and nothing
/// else: decoding and compositing read from it. It never measures time
/// itself — [`PlaybackScheduler::follow`] takes the master's reading — so it
/// is deterministic and testable without a real timer, and identical whether
/// the master is the audio callback or [`MonotonicClock`].
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
    /// The master clock used when no audio stream is playing.
    fallback: MonotonicClock,
    /// The master's last reading, in frames at `rate`, or `None` when the
    /// baseline is gone — a seek, a speed change, or a master that has just
    /// started publishing again. The next reading only re-establishes it, so
    /// a jump in the master is not counted as dropped frames.
    master: Option<i64>,
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
            fallback: MonotonicClock::new(rate),
            master: None,
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
        self.fallback.set_rate(rate);
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

    /// Moves the playhead, clamped to the sequence, and re-seeds the master
    /// baseline there so the next frame is a whole one.
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
    ///
    /// Whoever owns the audio transport seeks it to the same instant: the
    /// master defines sync, so moving the playhead means moving the master.
    pub fn seek_frames(&mut self, frame: i64) {
        self.frame = frame.clamp(0, self.last_frame_number());
        self.fallback.reset_to(self.frame);
        self.master = Some(self.frame);
    }

    /// Forgets the master's baseline without moving the playhead.
    ///
    /// The transport calls this when the master reading is about to jump for
    /// a reason that is not playback — the audio stream was reopened, or its
    /// clock was reset by a seek — so the jump is not counted as dropped
    /// frames.
    pub const fn resync_master(&mut self) {
        self.master = None;
    }

    /// The fallback master, for a caller that wants to read or re-seed it.
    #[must_use]
    pub const fn fallback_clock(&self) -> &MonotonicClock {
        &self.fallback
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
        self.fallback.reset_to(self.frame);
        self.master = Some(self.frame);
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
    /// again, so playback keeps time without polling. It is the fallback
    /// master's cadence: a caller following an audio clock repaints when the
    /// clock moves, not when this expires.
    #[must_use]
    pub fn time_until_next_frame(&self) -> Option<Duration> {
        self.is_playing()
            .then(|| self.fallback.time_until_next_frame())
    }

    /// Picks the frame that covers `master`, the media time the playback
    /// master has reached, and returns what that presentation did.
    ///
    /// This is where video follows audio (docs/PLAN.md §5.4): `master` is the
    /// audio clock's position during real playback, so the frame on screen is
    /// always the one the samples being heard belong to. The playhead lands
    /// where the master says rather than one frame on from the last one
    /// shown, so a master that has run ahead — decode fell behind, the app
    /// was not scheduled — drops the presentations in between and counts
    /// them instead of playing them late.
    ///
    /// Returns `None` while paused, and while the master is still inside the
    /// frame already on screen.
    pub fn follow(&mut self, master: RationalTime) -> Option<Tick> {
        if !self.is_playing() {
            return None;
        }
        let target = master
            .rescaled_to_rounding(self.rate, Rounding::Floor)
            .value();
        let baseline = self.master.replace(target);
        let dropped = match baseline {
            // A master reading in the same frame as the last one has nothing
            // new to show.
            Some(previous) if previous == target => return None,
            Some(previous) => {
                let per_frame = u64::try_from(self.speed.multiplier()).unwrap_or(1).max(1);
                let covered = previous.abs_diff(target) / per_frame;
                covered.saturating_sub(1)
            }
            // No baseline: the master has just been (re-)seeded, so the step
            // from wherever it was is not a dropped frame.
            None if target == self.frame => return None,
            None => 0,
        };
        if dropped > 0 {
            self.dropped = self.dropped.saturating_add(dropped);
            tracing::warn!(
                dropped,
                total = self.dropped,
                speed = self.speed.label(),
                "playback fell behind the master clock; frames dropped to stay in time"
            );
        }

        let before = self.frame;
        let (frame, wrapped, stopped) = self.landing(target);
        self.frame = frame;
        if stopped {
            self.speed = ShuttleSpeed::Paused;
            self.fallback.reset_to(frame);
            self.master = None;
        }
        Some(Tick {
            position: self.position(),
            frames_advanced: frame - before,
            dropped,
            wrapped,
            stopped,
        })
    }

    /// Runs the fallback master on for `elapsed` and follows it.
    ///
    /// This is what a caller with no audio stream to follow uses: a silent
    /// sequence, a shuttle speed nothing is playing at, or the headless
    /// engine. It is [`MonotonicClock::advance`] followed by
    /// [`PlaybackScheduler::follow`], and nothing else.
    pub fn advance(&mut self, elapsed: Duration) -> Option<Tick> {
        if !self.is_playing() {
            return None;
        }
        let master = self.fallback.advance(elapsed, self.speed);
        self.follow(master)
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

    #[test]
    fn the_frame_shown_is_the_one_that_covers_the_master_time() {
        let mut scheduler = scheduler(200);
        scheduler.play_forward();
        // A master reading at 48 kHz, a second and a half in: frame 36 at 24
        // fps, and the frames either side of the boundary confirm the choice
        // is the frame the time falls inside rather than the nearest one.
        let audio_rate = Rational::new(48_000, 1).unwrap();
        let tick = scheduler
            .follow(RationalTime::new(72_000, audio_rate))
            .unwrap();
        assert_eq!(tick.position, RationalTime::new(36, Rational::FPS_24));
        // Still inside frame 36: nothing new to present.
        assert_eq!(
            scheduler.follow(RationalTime::new(73_999, audio_rate)),
            None
        );
        let tick = scheduler
            .follow(RationalTime::new(74_000, audio_rate))
            .unwrap();
        assert_eq!(tick.frames_advanced, 1);
        assert_eq!(tick.position, RationalTime::new(37, Rational::FPS_24));
    }

    #[test]
    fn a_master_that_ran_ahead_drops_the_presentations_it_passed() {
        let mut scheduler = scheduler(200);
        scheduler.play_forward();
        let tick = scheduler
            .follow(RationalTime::new(5, Rational::FPS_24))
            .unwrap();
        assert_eq!(tick.frames_advanced, 5);
        assert_eq!(tick.dropped, 4);
        assert_eq!(scheduler.position_frames(), 5);
        assert_eq!(scheduler.dropped_frames(), 4);
    }

    #[test]
    fn a_master_that_jumped_for_a_seek_is_not_counted_as_dropped_frames() {
        let mut scheduler = scheduler(200);
        scheduler.play_forward();
        scheduler.follow(RationalTime::new(5, Rational::FPS_24));
        scheduler.resync_master();
        // The audio stream was reopened and its clock reset; the reading that
        // comes back is a long way off, but nothing was missed.
        let tick = scheduler
            .follow(RationalTime::new(120, Rational::FPS_24))
            .unwrap();
        assert_eq!(scheduler.position_frames(), 120);
        assert_eq!(tick.dropped, 0);
        assert_eq!(scheduler.dropped_frames(), 4);
    }

    #[test]
    fn a_paused_scheduler_ignores_the_master() {
        let mut scheduler = scheduler(200);
        assert_eq!(
            scheduler.follow(RationalTime::new(10, Rational::FPS_24)),
            None
        );
        assert_eq!(scheduler.position_frames(), 0);
    }

    #[test]
    fn following_the_master_wraps_the_loop_and_stops_at_the_end() {
        let mut scheduler = scheduler(48);
        let range = TimeRange::from_start_end(
            RationalTime::new(4, Rational::FPS_24),
            RationalTime::new(8, Rational::FPS_24),
        )
        .unwrap();
        scheduler.set_loop_range(Some(range)).unwrap();
        scheduler.seek_frames(4);
        scheduler.play_forward();
        let tick = scheduler
            .follow(RationalTime::new(8, Rational::FPS_24))
            .unwrap();
        assert!(tick.wrapped);
        assert_eq!(scheduler.position_frames(), 4);

        scheduler.set_loop_range(None).unwrap();
        let tick = scheduler
            .follow(RationalTime::new(400, Rational::FPS_24))
            .unwrap();
        assert!(tick.stopped);
        assert_eq!(scheduler.position_frames(), 47);
        assert!(!scheduler.is_playing());
    }

    #[test]
    fn an_audio_master_at_a_fractional_rate_never_drifts_a_frame() {
        // Ten minutes of 48 kHz samples read as 23.976 video: the frame
        // chosen is always the frame the sample count falls inside, with no
        // accumulated error at the end.
        let video = Rational::FPS_23_976;
        let audio = Rational::new(48_000, 1).unwrap();
        let mut scheduler = PlaybackScheduler::new(video);
        scheduler.set_duration(RationalTime::new(15_000, video));
        scheduler.play_forward();
        let mut samples: i64 = 0;
        while samples < 48_000 * 600 {
            samples += 512;
            scheduler.follow(RationalTime::new(samples, audio));
            let expected = RationalTime::new(samples, audio)
                .rescaled_to_rounding(video, Rounding::Floor)
                .value();
            assert_eq!(
                scheduler.position_frames(),
                expected.min(scheduler.last_frame_number()),
                "drifted at {samples} samples"
            );
        }
    }

    #[test]
    fn the_fallback_master_measures_media_time_from_wall_time() {
        let mut clock = MonotonicClock::new(Rational::FPS_24);
        assert_eq!(clock.position(), RationalTime::zero(Rational::FPS_24));
        // Half a frame is not a frame yet, and the part-frame is carried.
        clock.advance(Duration::from_millis(20), ShuttleSpeed::Forward1x);
        assert_eq!(clock.position().value(), 0);
        clock.advance(Duration::from_millis(22), ShuttleSpeed::Forward1x);
        assert_eq!(clock.position().value(), 1);
        // A paused master stands still however long the caller waited.
        clock.advance(Duration::from_secs(10), ShuttleSpeed::Paused);
        assert_eq!(clock.position().value(), 1);
        // Reverse runs it backwards, and the scheduler decides what that
        // means for the playhead.
        clock.reset_to(0);
        clock.advance(FRAME_24 * 3, ShuttleSpeed::Reverse2x);
        assert_eq!(clock.position().value(), -6);
    }

    #[test]
    fn the_fallback_master_re_expresses_itself_at_a_new_timebase() {
        let mut clock = MonotonicClock::new(Rational::FPS_24);
        clock.reset_to(12);
        let fps_48 = Rational::new(48, 1).unwrap();
        clock.set_rate(fps_48);
        assert_eq!(clock.rate(), fps_48);
        assert_eq!(clock.position(), RationalTime::new(24, fps_48));
    }

    #[test]
    fn advancing_is_the_fallback_master_followed() {
        // The two paths agree: running the fallback by hand and following it
        // lands exactly where advance does.
        let mut by_hand = scheduler(200);
        let mut advanced = scheduler(200);
        by_hand.play_forward();
        advanced.play_forward();
        let mut clock = MonotonicClock::new(Rational::FPS_24);
        for _ in 0..50 {
            let master = clock.advance(Duration::from_millis(30), ShuttleSpeed::Forward1x);
            by_hand.follow(master);
            advanced.advance(Duration::from_millis(30));
            assert_eq!(by_hand.position_frames(), advanced.position_frames());
        }
        assert_eq!(advanced.position_frames(), 36);
    }
}
