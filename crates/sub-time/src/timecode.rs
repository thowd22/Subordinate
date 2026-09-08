//! SMPTE timecode: drop-frame and non-drop-frame formatting and parsing.
//!
//! A [`Timecode`] is a label for a frame, never a duration: four unsigned
//! fields plus the [`TimecodeRate`] they are counted at. All conversions are
//! integer arithmetic on frame numbers, so a timecode round-trips exactly
//! across the whole 24-hour range at every supported rate.

use core::fmt;

use sub_core::{SubError, SubResult};

use crate::rational::Rational;
use crate::rational_time::{RationalTime, Rounding};

/// Stable error codes raised by timecode conversions.
pub mod codes {
    use sub_core::ErrorCode;

    /// The string is not four numeric fields separated by `:` or `;`, or its
    /// separators disagree with the rate's drop-frame flag.
    pub const TIMECODE_SYNTAX: ErrorCode = ErrorCode::from_static("time.timecode_syntax");
    /// A field is outside its range: hours over 23, minutes or seconds over
    /// 59, or a frame number at or above the nominal frame rate.
    pub const TIMECODE_OUT_OF_RANGE: ErrorCode =
        ErrorCode::from_static("time.timecode_out_of_range");
    /// The timecode names one of the labels that drop-frame counting skips,
    /// such as `00:01:00;00` at 29.97 drop-frame.
    pub const TIMECODE_DROPPED_FRAME: ErrorCode =
        ErrorCode::from_static("time.timecode_dropped_frame");
    /// The rate is not one timecode can label: it is neither a whole number of
    /// frames per second nor an NTSC `n000/1001` rate, or it is faster than
    /// 1000 fps.
    pub const TIMECODE_RATE_UNSUPPORTED: ErrorCode =
        ErrorCode::from_static("time.timecode_rate_unsupported");
    /// Drop-frame was requested at a rate that has no drop-frame counting:
    /// only NTSC rates whose nominal frame rate is a multiple of 30 (29.97,
    /// 59.94) drop frames.
    pub const TIMECODE_DROP_FRAME_UNSUPPORTED: ErrorCode =
        ErrorCode::from_static("time.timecode_drop_frame_unsupported");
}

/// The fastest rate timecode may be counted at, in nominal frames per second.
const MAX_NOMINAL_FPS: u32 = 1000;

/// Minutes in 24 hours.
const MINUTES_PER_24H: i64 = 24 * 60;

/// Narrows a value the caller has already bounded to `0..=u32::MAX`.
fn narrow_u32(value: i64) -> u32 {
    debug_assert!((0..=i64::from(u32::MAX)).contains(&value));
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// A frame rate together with the counting rule timecode uses at it.
///
/// The nominal frame rate is the whole number of frames a timecode second
/// holds: 24 for `24000/1001`, 30 for `30000/1001`, 60 for `60000/1001`. At an
/// NTSC rate the clock therefore runs 0.1% slow, and *drop-frame* counting
/// corrects it by skipping frame labels: the first `nominal / 15` labels of
/// every minute except every tenth minute do not exist.
///
/// ```
/// use sub_time::{Rational, TimecodeRate};
///
/// let df = TimecodeRate::new(Rational::FPS_29_97, true).unwrap();
/// assert_eq!(df.nominal_fps(), 30);
/// assert_eq!(df.frames_per_24h(), 2_589_408);
///
/// // 25 fps has nothing to correct, so drop-frame is rejected.
/// assert!(TimecodeRate::new(Rational::FPS_25, true).is_err());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimecodeRate {
    rate: Rational,
    nominal_fps: u32,
    drop_frame: bool,
}

impl TimecodeRate {
    /// Creates a timecode rate.
    ///
    /// # Errors
    ///
    /// Returns [`codes::TIMECODE_RATE_UNSUPPORTED`] if `rate` is neither a
    /// whole number of frames per second nor an NTSC `n000/1001` rate, or if
    /// its nominal rate exceeds 1000 fps, and
    /// [`codes::TIMECODE_DROP_FRAME_UNSUPPORTED`] if `drop_frame` is set at a
    /// rate that does not drop frames.
    pub fn new(rate: Rational, drop_frame: bool) -> SubResult<Self> {
        let nominal_fps = nominal_fps_of(rate).ok_or_else(|| {
            SubError::new(
                codes::TIMECODE_RATE_UNSUPPORTED,
                "rate cannot be labelled with timecode",
            )
            .with_detail("rate", rate.to_string())
        })?;
        if drop_frame && !drops_frames(rate, nominal_fps) {
            return Err(SubError::new(
                codes::TIMECODE_DROP_FRAME_UNSUPPORTED,
                "rate has no drop-frame counting",
            )
            .with_detail("rate", rate.to_string()));
        }
        Ok(Self {
            rate,
            nominal_fps,
            drop_frame,
        })
    }

    /// Creates a non-drop-frame timecode rate.
    ///
    /// # Errors
    ///
    /// As [`Self::new`], minus the drop-frame case.
    pub fn non_drop(rate: Rational) -> SubResult<Self> {
        Self::new(rate, false)
    }

    /// Creates a drop-frame timecode rate.
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn drop_frame(rate: Rational) -> SubResult<Self> {
        Self::new(rate, true)
    }

    /// True if `rate` has drop-frame counting: an NTSC rate whose nominal
    /// frame rate is a multiple of 30.
    pub fn rate_drops_frames(rate: Rational) -> bool {
        nominal_fps_of(rate).is_some_and(|nominal| drops_frames(rate, nominal))
    }

    /// The exact underlying rate.
    pub const fn rate(self) -> Rational {
        self.rate
    }

    /// The whole number of frames in one timecode second.
    pub const fn nominal_fps(self) -> u32 {
        self.nominal_fps
    }

    /// True if frame labels are dropped to track wall-clock time.
    pub const fn is_drop_frame(self) -> bool {
        self.drop_frame
    }

    /// Frame labels dropped at the start of each dropping minute: zero for
    /// non-drop-frame, two at 29.97, four at 59.94.
    fn dropped_per_minute(self) -> i64 {
        if self.drop_frame {
            i64::from(self.nominal_fps / 15)
        } else {
            0
        }
    }

    /// The number of distinct frame labels in one 24-hour timecode day, which
    /// is where [`Timecode::from_frame_number`] wraps.
    pub fn frames_per_24h(self) -> i64 {
        let nominal = i64::from(self.nominal_fps);
        // Nine minutes in ten drop frames; 24 hours holds 144 tenth minutes.
        nominal * 60 * MINUTES_PER_24H - self.dropped_per_minute() * (MINUTES_PER_24H - 144)
    }
}

/// The nominal (whole) frame rate of `rate`, or `None` if timecode cannot
/// label it.
fn nominal_fps_of(rate: Rational) -> Option<u32> {
    let nominal = if rate.is_integral() {
        rate.numerator()
    } else if rate.denominator() == 1001 && rate.numerator().is_multiple_of(1000) {
        rate.numerator() / 1000
    } else {
        return None;
    };
    if nominal == 0 || nominal > MAX_NOMINAL_FPS {
        return None;
    }
    Some(nominal)
}

/// True if drop-frame counting is defined at `rate`.
fn drops_frames(rate: Rational, nominal_fps: u32) -> bool {
    !rate.is_integral() && nominal_fps.is_multiple_of(30)
}

/// An SMPTE timecode label: `HH:MM:SS:FF`, or `HH;MM;SS;FF` when drop-frame.
///
/// Fields are always in range for the rate, so a `Timecode` that exists is one
/// that can be displayed and converted. Construction from a frame number wraps
/// at 24 hours the way a timecode counter does.
///
/// ```
/// use sub_time::{Rational, Timecode, TimecodeRate};
///
/// let df = TimecodeRate::drop_frame(Rational::FPS_29_97).unwrap();
/// // Drop-frame counting has caught up with wall clock by the tenth minute.
/// assert_eq!(Timecode::from_frame_number(17_982, df).to_string(), "00;10;00;00");
///
/// let parsed = Timecode::parse("00;01;00;02", df).unwrap();
/// assert_eq!(parsed.to_frame_number(), 1_800);
/// // The two labels that minute skips do not exist.
/// assert!(Timecode::parse("00;01;00;00", df).is_err());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Timecode {
    rate: TimecodeRate,
    hours: u32,
    minutes: u32,
    seconds: u32,
    frames: u32,
}

impl Timecode {
    /// Creates a timecode from its four fields.
    ///
    /// # Errors
    ///
    /// Returns [`codes::TIMECODE_OUT_OF_RANGE`] if a field is out of range for
    /// `rate`, and [`codes::TIMECODE_DROPPED_FRAME`] if the fields name a
    /// label that drop-frame counting skips.
    pub fn new(
        rate: TimecodeRate,
        hours: u32,
        minutes: u32,
        seconds: u32,
        frames: u32,
    ) -> SubResult<Self> {
        let out_of_range = |field: &'static str, value: u32, limit: u32| {
            SubError::new(
                codes::TIMECODE_OUT_OF_RANGE,
                format!("timecode {field} field is out of range"),
            )
            .with_detail("field", field)
            .with_detail("value", value)
            .with_detail("limit", limit)
        };
        if hours > 23 {
            return Err(out_of_range("hours", hours, 23));
        }
        if minutes > 59 {
            return Err(out_of_range("minutes", minutes, 59));
        }
        if seconds > 59 {
            return Err(out_of_range("seconds", seconds, 59));
        }
        if frames >= rate.nominal_fps() {
            return Err(out_of_range("frames", frames, rate.nominal_fps() - 1));
        }
        let dropped = rate.dropped_per_minute();
        if dropped > 0 && seconds == 0 && !minutes.is_multiple_of(10) && i64::from(frames) < dropped
        {
            return Err(SubError::new(
                codes::TIMECODE_DROPPED_FRAME,
                "drop-frame counting skips this timecode",
            )
            .with_detail("minutes", minutes)
            .with_detail("frames", frames));
        }
        Ok(Self {
            rate,
            hours,
            minutes,
            seconds,
            frames,
        })
    }

    /// The timecode labelling `frame`, counted from `00:00:00:00`.
    ///
    /// Frame numbers wrap at 24 hours, in both directions: one frame before
    /// zero is the last frame of the day.
    pub fn from_frame_number(frame: i64, rate: TimecodeRate) -> Self {
        let nominal = i64::from(rate.nominal_fps());
        let mut counted = frame.rem_euclid(rate.frames_per_24h());
        let dropped = rate.dropped_per_minute();
        if dropped > 0 {
            // Re-insert the labels the counting skipped, so what remains is a
            // plain count of labels at the nominal rate.
            let per_dropping_minute = nominal * 60 - dropped;
            let per_ten_minutes = nominal * 600 - 9 * dropped;
            let blocks = counted / per_ten_minutes;
            let within = counted % per_ten_minutes;
            // The first minute of each ten-minute block drops nothing.
            let dropping_minutes = (within - dropped).max(0) / per_dropping_minute;
            counted += 9 * dropped * blocks + dropped * dropping_minutes;
        }
        let frames = counted % nominal;
        let total_seconds = counted / nominal;
        Self {
            rate,
            hours: narrow_u32(total_seconds / 3600),
            minutes: narrow_u32(total_seconds / 60 % 60),
            seconds: narrow_u32(total_seconds % 60),
            frames: narrow_u32(frames),
        }
    }

    /// The number of frames from `00:00:00:00` to this timecode, in
    /// `0..rate.frames_per_24h()`.
    pub fn to_frame_number(self) -> i64 {
        let nominal = i64::from(self.rate.nominal_fps());
        let total_minutes = i64::from(self.hours) * 60 + i64::from(self.minutes);
        let counted =
            (total_minutes * 60 + i64::from(self.seconds)) * nominal + i64::from(self.frames);
        // Every minute but each tenth has dropped its labels by now.
        counted - self.rate.dropped_per_minute() * (total_minutes - total_minutes / 10)
    }

    /// The timecode labelling the frame `time` falls on.
    ///
    /// `time` is rescaled to the timecode rate, rounding to the nearest frame
    /// with ties away from zero, then wrapped into the 24-hour day.
    ///
    /// # Errors
    ///
    /// Returns [`codes::TIMECODE_OUT_OF_RANGE`] if the rescaled frame number
    /// does not fit in an `i64`.
    pub fn from_rational_time(time: RationalTime, rate: TimecodeRate) -> SubResult<Self> {
        let frames = time
            .checked_rescaled_to_rounding(rate.rate(), Rounding::Nearest)
            .ok_or_else(|| {
                SubError::new(
                    codes::TIMECODE_OUT_OF_RANGE,
                    "time is too large to label with timecode",
                )
                .with_detail("value", time.value())
                .with_detail("rate", time.rate().to_string())
            })?;
        Ok(Self::from_frame_number(frames.value(), rate))
    }

    /// This timecode as a [`RationalTime`] at the timecode rate.
    pub fn to_rational_time(self) -> RationalTime {
        RationalTime::new(self.to_frame_number(), self.rate.rate())
    }

    /// Parses `HH:MM:SS:FF` (or `HH;MM;SS;FF` at a drop-frame rate) at `rate`.
    ///
    /// Fields may be written with one or two digits. A drop-frame rate accepts
    /// either separator; a non-drop-frame rate accepts only `:`, since `;`
    /// asserts a counting rule the rate does not use.
    ///
    /// # Errors
    ///
    /// Returns [`codes::TIMECODE_SYNTAX`] if the string is not four numeric
    /// fields, or uses `;` at a non-drop-frame rate; otherwise the errors of
    /// [`Self::new`].
    pub fn parse(text: &str, rate: TimecodeRate) -> SubResult<Self> {
        let syntax = |reason: &'static str| {
            SubError::new(codes::TIMECODE_SYNTAX, "malformed timecode")
                .with_detail("text", text)
                .with_detail("reason", reason)
        };
        let mut fields = [0_u32; 4];
        let mut index = 0_usize;
        let mut digits = 0_usize;
        let mut saw_drop_separator = false;
        for ch in text.chars() {
            match ch {
                '0'..='9' => {
                    if digits == 2 {
                        return Err(syntax("a field has more than two digits"));
                    }
                    let digit = u32::from(ch) - u32::from('0');
                    fields[index] = fields[index] * 10 + digit;
                    digits += 1;
                }
                ':' | ';' => {
                    if digits == 0 {
                        return Err(syntax("a field is empty"));
                    }
                    if index == 3 {
                        return Err(syntax("more than four fields"));
                    }
                    saw_drop_separator |= ch == ';';
                    index += 1;
                    digits = 0;
                }
                _ => return Err(syntax("unexpected character")),
            }
        }
        if index != 3 || digits == 0 {
            return Err(syntax("expected four fields HH:MM:SS:FF"));
        }
        if saw_drop_separator && !rate.is_drop_frame() {
            return Err(syntax("drop-frame separator at a non-drop-frame rate"));
        }
        Self::new(rate, fields[0], fields[1], fields[2], fields[3])
    }

    /// The rate this timecode is counted at.
    pub const fn rate(self) -> TimecodeRate {
        self.rate
    }

    /// The hours field, `0..=23`.
    pub const fn hours(self) -> u32 {
        self.hours
    }

    /// The minutes field, `0..=59`.
    pub const fn minutes(self) -> u32 {
        self.minutes
    }

    /// The seconds field, `0..=59`.
    pub const fn seconds(self) -> u32 {
        self.seconds
    }

    /// The frames field, below the nominal frame rate.
    pub const fn frames(self) -> u32 {
        self.frames
    }
}

impl fmt::Display for Timecode {
    /// Writes `HH:MM:SS:FF`, or `HH;MM;SS;FF` when the rate is drop-frame.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sep = if self.rate.is_drop_frame() { ';' } else { ':' };
        write!(
            f,
            "{:02}{sep}{:02}{sep}{:02}{sep}{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use super::{Timecode, TimecodeRate, codes};
    use crate::{Rational, RationalTime};

    fn ndf(rate: Rational) -> TimecodeRate {
        TimecodeRate::non_drop(rate).unwrap()
    }

    fn df(rate: Rational) -> TimecodeRate {
        TimecodeRate::drop_frame(rate).unwrap()
    }

    /// Every rate the task requires, as (label, rate).
    fn all_rates() -> Vec<(&'static str, TimecodeRate)> {
        vec![
            ("23.976", ndf(Rational::FPS_23_976)),
            ("24", ndf(Rational::FPS_24)),
            ("25", ndf(Rational::FPS_25)),
            ("29.97 NDF", ndf(Rational::FPS_29_97)),
            ("29.97 DF", df(Rational::FPS_29_97)),
            ("30", ndf(Rational::FPS_30)),
            ("50", ndf(Rational::FPS_50)),
            ("59.94 NDF", ndf(Rational::FPS_59_94)),
            ("59.94 DF", df(Rational::FPS_59_94)),
            ("60", ndf(Rational::FPS_60)),
        ]
    }

    #[test]
    fn nominal_rates_and_day_lengths() {
        assert_eq!(ndf(Rational::FPS_23_976).nominal_fps(), 24);
        assert_eq!(ndf(Rational::FPS_29_97).nominal_fps(), 30);
        assert_eq!(ndf(Rational::FPS_59_94).nominal_fps(), 60);
        assert_eq!(ndf(Rational::FPS_25).frames_per_24h(), 2_160_000);
        assert_eq!(ndf(Rational::FPS_29_97).frames_per_24h(), 2_592_000);
        assert_eq!(df(Rational::FPS_29_97).frames_per_24h(), 2_589_408);
        assert_eq!(df(Rational::FPS_59_94).frames_per_24h(), 5_178_816);
    }

    #[test]
    fn unsupported_rates_are_rejected() {
        let odd = Rational::new(24_001, 1001).unwrap();
        let err = TimecodeRate::non_drop(odd).unwrap_err();
        assert_eq!(err.code.as_str(), "time.timecode_rate_unsupported");

        let fast = Rational::from_integer(48_000).unwrap();
        assert_eq!(
            TimecodeRate::non_drop(fast).unwrap_err().code.as_str(),
            codes::TIMECODE_RATE_UNSUPPORTED.as_str()
        );
    }

    #[test]
    fn drop_frame_only_at_ntsc_thirty_multiples() {
        assert!(TimecodeRate::rate_drops_frames(Rational::FPS_29_97));
        assert!(TimecodeRate::rate_drops_frames(Rational::FPS_59_94));
        assert!(!TimecodeRate::rate_drops_frames(Rational::FPS_23_976));
        assert!(!TimecodeRate::rate_drops_frames(Rational::FPS_30));

        for rate in [
            Rational::FPS_23_976,
            Rational::FPS_24,
            Rational::FPS_25,
            Rational::FPS_30,
            Rational::FPS_50,
            Rational::FPS_60,
        ] {
            let err = TimecodeRate::drop_frame(rate).unwrap_err();
            assert_eq!(err.code.as_str(), "time.timecode_drop_frame_unsupported");
        }
    }

    #[test]
    fn known_drop_frame_vectors_at_29_97() {
        let rate = df(Rational::FPS_29_97);
        // The classic vector: ten minutes of drop-frame is 17982 frames.
        assert_eq!(
            Timecode::from_frame_number(17_982, rate).to_string(),
            "00;10;00;00"
        );
        assert_eq!(
            Timecode::parse("00;10;00;00", rate)
                .unwrap()
                .to_frame_number(),
            17_982
        );

        // The first minute skips ;00 and ;01, so 1800 is 00;01;00;02.
        assert_eq!(
            Timecode::from_frame_number(1_799, rate).to_string(),
            "00;00;59;29"
        );
        assert_eq!(
            Timecode::from_frame_number(1_800, rate).to_string(),
            "00;01;00;02"
        );
        assert_eq!(
            Timecode::from_frame_number(1_801, rate).to_string(),
            "00;01;00;03"
        );

        // Minute nine drops, minute ten does not.
        assert_eq!(
            Timecode::from_frame_number(17_981, rate).to_string(),
            "00;09;59;29"
        );
        assert_eq!(
            Timecode::from_frame_number(17_983, rate).to_string(),
            "00;10;00;01"
        );

        // An hour of drop-frame is six ten-minute blocks.
        assert_eq!(
            Timecode::from_frame_number(107_892, rate).to_string(),
            "01;00;00;00"
        );
        // The last label of the day, and the wrap back to zero.
        let day = rate.frames_per_24h();
        assert_eq!(
            Timecode::from_frame_number(day - 1, rate).to_string(),
            "23;59;59;29"
        );
        assert_eq!(
            Timecode::from_frame_number(day, rate).to_string(),
            "00;00;00;00"
        );
        assert_eq!(
            Timecode::from_frame_number(-1, rate).to_string(),
            "23;59;59;29"
        );
    }

    #[test]
    fn known_drop_frame_vectors_at_59_94() {
        let rate = df(Rational::FPS_59_94);
        // Four labels dropped per dropping minute.
        assert_eq!(
            Timecode::from_frame_number(3_599, rate).to_string(),
            "00;00;59;59"
        );
        assert_eq!(
            Timecode::from_frame_number(3_600, rate).to_string(),
            "00;01;00;04"
        );
        assert_eq!(
            Timecode::from_frame_number(35_964, rate).to_string(),
            "00;10;00;00"
        );
        assert_eq!(
            Timecode::from_frame_number(215_784, rate).to_string(),
            "01;00;00;00"
        );
    }

    #[test]
    fn non_drop_frame_counting_is_plain() {
        let rate = ndf(Rational::FPS_29_97);
        assert_eq!(
            Timecode::from_frame_number(1_800, rate).to_string(),
            "00:01:00:00"
        );
        assert_eq!(
            Timecode::from_frame_number(17_982, rate).to_string(),
            "00:09:59:12"
        );
        assert_eq!(
            Timecode::from_frame_number(2_591_999, rate).to_string(),
            "23:59:59:29"
        );

        let film = ndf(Rational::FPS_23_976);
        assert_eq!(
            Timecode::from_frame_number(24, film).to_string(),
            "00:00:01:00"
        );
        assert_eq!(
            Timecode::parse("01:00:00:00", film)
                .unwrap()
                .to_frame_number(),
            86_400
        );
    }

    #[test]
    fn dropped_labels_are_rejected() {
        let rate = df(Rational::FPS_29_97);
        for text in ["00;01;00;00", "00;01;00;01", "00;59;00;01"] {
            let err = Timecode::parse(text, rate).unwrap_err();
            assert_eq!(err.code.as_str(), "time.timecode_dropped_frame");
        }
        // Tenth minutes keep all their labels.
        assert!(Timecode::parse("00;10;00;00", rate).is_ok());
        assert!(Timecode::parse("00;20;00;01", rate).is_ok());
        // As does every second after the first.
        assert!(Timecode::parse("00;01;01;00", rate).is_ok());

        let fast = df(Rational::FPS_59_94);
        assert_eq!(
            Timecode::parse("00;01;00;03", fast)
                .unwrap_err()
                .code
                .as_str(),
            "time.timecode_dropped_frame"
        );
        assert!(Timecode::parse("00;01;00;04", fast).is_ok());
    }

    #[test]
    fn out_of_range_fields_are_rejected() {
        let rate = ndf(Rational::FPS_25);
        for text in ["24:00:00:00", "00:60:00:00", "00:00:60:00", "00:00:00:25"] {
            let err = Timecode::parse(text, rate).unwrap_err();
            assert_eq!(err.code.as_str(), "time.timecode_out_of_range");
        }
        assert!(Timecode::parse("23:59:59:24", rate).is_ok());
        assert!(Timecode::new(rate, 0, 0, 0, 25).is_err());
    }

    #[test]
    fn malformed_strings_are_rejected() {
        let rate = ndf(Rational::FPS_25);
        for text in [
            "",
            "00:00:00",
            "00:00:00:00:00",
            "00:00::00",
            "00:00:00:",
            "000:00:00:00",
            "0a:00:00:00",
            "-00:00:00:01",
            " 00:00:00:00",
        ] {
            let err = Timecode::parse(text, rate).unwrap_err();
            assert_eq!(err.code.as_str(), "time.timecode_syntax", "for {text:?}");
        }
        // One-digit fields are accepted.
        assert_eq!(
            Timecode::parse("1:2:3:4", rate).unwrap().to_string(),
            "01:02:03:04"
        );
    }

    #[test]
    fn separators_must_match_the_counting_rule() {
        let ndf_rate = ndf(Rational::FPS_29_97);
        let err = Timecode::parse("00;00;00;00", ndf_rate).unwrap_err();
        assert_eq!(err.code.as_str(), "time.timecode_syntax");

        // A drop-frame rate accepts either separator but always writes ';'.
        let df_rate = df(Rational::FPS_29_97);
        assert_eq!(
            Timecode::parse("00:10:00:00", df_rate).unwrap().to_string(),
            "00;10;00;00"
        );
    }

    #[test]
    fn rational_time_round_trip() {
        let rate = df(Rational::FPS_29_97);
        let tc = Timecode::from_frame_number(17_982, rate);
        let time = tc.to_rational_time();
        assert_eq!(time.value(), 17_982);
        assert_eq!(time.rate(), Rational::FPS_29_97);
        assert_eq!(Timecode::from_rational_time(time, rate).unwrap(), tc);

        // A 24 fps position labelled at 23.976 keeps its frame index.
        let film = ndf(Rational::FPS_23_976);
        let at_24 = RationalTime::from_frames(48, Rational::FPS_24);
        assert_eq!(
            Timecode::from_rational_time(at_24, film)
                .unwrap()
                .to_string(),
            "00:00:02:00"
        );

        // Ten seconds of wall clock at 29.97 drop-frame stays under a label of
        // ten seconds, because the labels themselves run slow.
        let ten_seconds = RationalTime::from_seconds(10);
        assert_eq!(
            Timecode::from_rational_time(ten_seconds, rate)
                .unwrap()
                .to_string(),
            "00;00;10;00"
        );
    }

    #[test]
    fn from_rational_time_rejects_overflow() {
        let rate = ndf(Rational::FPS_30);
        let huge = RationalTime::from_frames(i64::MAX, Rational::ONE);
        let err = Timecode::from_rational_time(huge, rate).unwrap_err();
        assert_eq!(err.code.as_str(), "time.timecode_out_of_range");
    }

    #[test]
    fn frame_number_round_trips_over_a_full_day_at_every_rate() {
        for (label, rate) in all_rates() {
            let day = rate.frames_per_24h();
            let mut text = String::with_capacity(16);
            for frame in 0..day {
                let tc = Timecode::from_frame_number(frame, rate);
                assert_eq!(tc.to_frame_number(), frame, "{label} frame {frame}");
                text.clear();
                write!(&mut text, "{tc}").unwrap();
                let parsed = Timecode::parse(&text, rate)
                    .unwrap_or_else(|err| panic!("{label} {text}: {err}"));
                assert_eq!(parsed, tc, "{label} frame {frame}");
                assert_eq!(parsed.to_frame_number(), frame, "{label} frame {frame}");
            }
            // The day boundary wraps rather than growing an extra hour.
            assert_eq!(
                Timecode::from_frame_number(day, rate),
                Timecode::from_frame_number(0, rate),
                "{label} wrap"
            );
        }
    }
}
