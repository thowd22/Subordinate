//! The detector: which parts of a sound are silence.
//!
//! Kept apart from the component bindings so it can be unit-tested on the host
//! triple and read without any WIT in the way, exactly like `plugins/gain`'s
//! DSP. It measures in audio frames — whole samples, counted from the start of
//! the file — because that is the media's own exact timebase; turning a frame
//! count into a `rational-time` is a division the caller never has to do, and
//! no float ever touches a timeline.
//!
//! The measurement is a moving RMS over a short window. Peak would call a
//! single stray sample "not silence", and averaging over the whole file would
//! call a pause in loud material "not silence"; a 20 ms RMS window is what a
//! listener hears as a level.

/// One span of the media, in audio frames, half-open: `[start, end)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    /// The first frame in the span.
    pub start: u64,
    /// The first frame after the span.
    pub end: u64,
}

impl Span {
    /// How many frames the span covers.
    pub fn len(self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    /// Whether the span covers nothing.
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
}

/// What counts as silence, in frames at the media's own sample rate.
///
/// [`Settings::at_rate`] is how the seconds a user types become these, once,
/// where the sample rate is known.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// The level at or below which a window is silent, in linear amplitude.
    pub threshold: f32,
    /// The RMS window, in frames; never zero.
    pub window: u64,
    /// How long a run of silent windows must be before it is worth cutting.
    pub minimum: u64,
    /// How much silence to leave at each end of a cut, so speech does not
    /// start or stop abruptly.
    pub padding: u64,
}

/// The seconds a caller asks in, before they are turned into frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// The level at or below which a window is silent, in decibels full
    /// scale. `-50.0` is a good default for dialogue.
    pub threshold_db: f32,
    /// The RMS window in seconds.
    pub window_seconds: f32,
    /// The shortest silence worth cutting, in seconds.
    pub minimum_seconds: f32,
    /// The air left at each end of a cut, in seconds.
    pub padding_seconds: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            threshold_db: -50.0,
            window_seconds: 0.02,
            minimum_seconds: 0.5,
            padding_seconds: 0.1,
        }
    }
}

impl Options {
    /// Turns seconds into frame counts at `rate`, clamping each to something
    /// the detector can use: the window is at least one frame, and a negative
    /// or non-finite duration reads as zero.
    pub fn at_rate(self, rate: u32) -> Settings {
        Settings {
            threshold: linear_from_dbfs(self.threshold_db),
            window: frames(self.window_seconds, rate).max(1),
            minimum: frames(self.minimum_seconds, rate),
            padding: frames(self.padding_seconds, rate),
        }
    }
}

/// Linear amplitude for a level in decibels full scale.
///
/// A level at or below -300 dBFS, or one that is not a number, is exact zero:
/// a threshold of "digital silence" must not be a very small positive number
/// that a denormal creeps under.
pub fn linear_from_dbfs(decibels: f32) -> f32 {
    if !decibels.is_finite() || decibels <= -300.0 {
        return 0.0;
    }
    10.0_f32.powf(decibels / 20.0)
}

/// Seconds at `rate`, rounded to the nearest whole frame.
///
/// Rounded rather than truncated because `0.01_f32` is a shade under a
/// hundredth: truncating would make a 10 ms window nine frames long and put
/// every span the detector reports on a nine-frame grid.
fn frames(seconds: f32, rate: u32) -> u64 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    (f64::from(seconds) * f64::from(rate)).round() as u64
}

/// Finds the silent spans of `samples`, in frames.
///
/// `samples` is one mono value per audio frame — [`crate::wav::Wav::frames`]
/// yields exactly that. The result is in ascending order, the spans do not
/// touch, and each is at least `settings.minimum` long *before* padding is
/// taken off it, so raising the padding never turns a short silence into a
/// cut.
pub fn detect(samples: impl Iterator<Item = f32>, settings: &Settings) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut run: Option<u64> = None;
    let mut frame = 0_u64;
    let mut window_frames = 0_u64;
    let mut sum_squares = 0.0_f64;

    let close = |run: &mut Option<u64>, end: u64, spans: &mut Vec<Span>| {
        if let Some(start) = run.take() {
            let span = Span { start, end };
            if span.len() >= settings.minimum
                && let Some(span) = pad(span, settings.padding)
            {
                spans.push(span);
            }
        }
    };

    for sample in samples {
        sum_squares += f64::from(sample) * f64::from(sample);
        window_frames += 1;
        frame += 1;
        if window_frames == settings.window {
            let rms = (sum_squares / window_frames as f64).sqrt();
            if rms <= f64::from(settings.threshold) {
                run.get_or_insert(frame - window_frames);
            } else {
                close(&mut run, frame - window_frames, &mut spans);
            }
            window_frames = 0;
            sum_squares = 0.0;
        }
    }

    // The tail of the file is a short window; judging it on its own terms is
    // what lets a file that ends in silence end in a cut.
    if window_frames > 0 {
        let rms = (sum_squares / window_frames as f64).sqrt();
        if rms <= f64::from(settings.threshold) {
            run.get_or_insert(frame - window_frames);
        } else {
            close(&mut run, frame - window_frames, &mut spans);
        }
    }
    close(&mut run, frame, &mut spans);
    spans
}

/// Takes `padding` frames off each end of `span`, or drops it if that leaves
/// nothing.
fn pad(span: Span, padding: u64) -> Option<Span> {
    let start = span.start.saturating_add(padding);
    let end = span.end.saturating_sub(padding);
    (end > start).then_some(Span { start, end })
}

#[cfg(test)]
mod tests {
    use super::{Options, Span, detect, linear_from_dbfs};

    /// `quiet` frames of digital silence, then `loud` frames of full-scale
    /// tone, repeated as the pattern says.
    fn signal(pattern: &[(u64, bool)]) -> Vec<f32> {
        let mut samples = Vec::new();
        for &(frames, loud) in pattern {
            for frame in 0..frames {
                samples.push(if loud {
                    if frame % 2 == 0 { 0.5 } else { -0.5 }
                } else {
                    0.0
                });
            }
        }
        samples
    }

    /// Options in frames at 1000 Hz: a 10-frame window, a 100-frame minimum
    /// and no padding, so a test counts in round numbers.
    fn settings(minimum_seconds: f32, padding_seconds: f32) -> super::Settings {
        Options {
            threshold_db: -50.0,
            window_seconds: 0.01,
            minimum_seconds,
            padding_seconds,
        }
        .at_rate(1_000)
    }

    #[test]
    fn silence_between_two_loud_passages_is_found_at_its_own_edges() {
        let samples = signal(&[(500, true), (1_000, false), (500, true)]);
        let spans = detect(samples.into_iter(), &settings(0.1, 0.0));
        assert_eq!(
            spans,
            vec![Span {
                start: 500,
                end: 1_500
            }]
        );
    }

    #[test]
    fn a_silence_shorter_than_the_minimum_is_left_alone() {
        let samples = signal(&[(500, true), (60, false), (500, true)]);
        assert!(detect(samples.into_iter(), &settings(0.1, 0.0)).is_empty());
    }

    #[test]
    fn padding_leaves_air_at_both_ends_and_can_cancel_a_cut() {
        let samples = signal(&[(200, true), (1_000, false), (200, true)]);
        let spans = detect(samples.into_iter(), &settings(0.1, 0.2));
        assert_eq!(
            spans,
            vec![Span {
                start: 400,
                end: 1_000
            }]
        );

        // 300 frames of silence, 200 of padding at each end: nothing left.
        let samples = signal(&[(200, true), (300, false), (200, true)]);
        assert!(detect(samples.into_iter(), &settings(0.1, 0.2)).is_empty());
    }

    #[test]
    fn silence_at_the_head_and_the_tail_is_found_too() {
        let samples = signal(&[(400, false), (200, true), (400, false)]);
        let spans = detect(samples.into_iter(), &settings(0.1, 0.0));
        assert_eq!(
            spans,
            vec![
                Span { start: 0, end: 400 },
                Span {
                    start: 600,
                    end: 1_000
                },
            ]
        );
    }

    #[test]
    fn a_quiet_room_is_silence_and_a_loud_one_is_not() {
        // -60 dBFS room tone, under a -50 dBFS threshold.
        let room = vec![0.001_f32; 1_000];
        assert_eq!(detect(room.into_iter(), &settings(0.1, 0.0)).len(), 1);
        // -20 dBFS, over it.
        let room = vec![0.1_f32; 1_000];
        assert!(detect(room.into_iter(), &settings(0.1, 0.0)).is_empty());
    }

    #[test]
    fn an_empty_file_and_an_impossible_threshold_are_survivable() {
        assert!(detect(std::iter::empty(), &settings(0.1, 0.0)).is_empty());
        assert_eq!(linear_from_dbfs(0.0), 1.0);
        assert_eq!(linear_from_dbfs(f32::NAN), 0.0);
        assert_eq!(linear_from_dbfs(f32::NEG_INFINITY), 0.0);
        assert!((linear_from_dbfs(-6.0) - 0.501_187).abs() < 1e-5);
        // Zero seconds of window still leaves a window of one frame.
        let settings = Options {
            window_seconds: 0.0,
            ..Options::default()
        }
        .at_rate(48_000);
        assert_eq!(settings.window, 1);
    }
}
