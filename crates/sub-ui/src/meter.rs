//! Level meters: the widget the track headers and the viewer draw, and the
//! per-meter state that gives it a peak hold and a clip indicator.
//!
//! The audio callback publishes a [`MeterLevels`] per track and one for the
//! master into a [`MeterBank`](sub_audio::MeterBank); everything here is the
//! reader's side of that, and runs on the UI thread. A [`MeterState`] is a
//! handful of floats: it takes one measurement per frame with the seconds
//! since the last one, holds the peak for a moment before letting it fall at a
//! fixed rate, and latches the clip indicator so that a single clipped block
//! between two frames is still seen.
//!
//! The scale is decibels: [`MIN_DB`] at the left of the bar, full scale at the
//! right, which is what makes quiet material readable at all.
//!
//! ```
//! use sub_audio::MeterLevels;
//! use sub_ui::meter::MeterState;
//!
//! let mut meter = MeterState::new();
//! meter.update(MeterLevels::new(1.2, 0.5), 1.0 / 60.0);
//! assert!(meter.clipping(), "a clipped block latches the indicator");
//!
//! // Silence afterwards: the bar falls at once, the hold falls slowly.
//! meter.update(MeterLevels::SILENT, 1.0 / 60.0);
//! assert_eq!(meter.levels(), MeterLevels::SILENT);
//! assert!(meter.peak_hold_db() > -1.0);
//! ```

use eframe::egui::{Color32, Painter, Rect, Stroke, Ui, Visuals, pos2, vec2};
use sub_audio::MeterLevels;

/// The quietest level a meter draws. Anything below is an empty bar.
pub const MIN_DB: f32 = -60.0;

/// How long the peak hold stays where it was put, in seconds.
pub const PEAK_HOLD_SECONDS: f32 = 1.5;

/// How fast the peak hold falls once the hold has expired, in decibels per
/// second.
pub const PEAK_FALL_DB_PER_SECOND: f32 = 24.0;

/// How long the clip indicator stays lit after the last clipped block, in
/// seconds.
pub const CLIP_HOLD_SECONDS: f32 = 2.0;

/// The level above which the bar warns, in decibels.
const WARN_DB: f32 = -6.0;

/// The width of the clip indicator at the loud end of the bar, in points.
const CLIP_WIDTH: f32 = 4.0;

/// The bar below the warning level.
pub const NORMAL_COLOR: Color32 = Color32::from_rgb(64, 168, 96);

/// The bar between the warning level and full scale.
pub const WARN_COLOR: Color32 = Color32::from_rgb(210, 168, 60);

/// The clip indicator, lit, and the bar behind a clipped level.
pub const CLIP_COLOR: Color32 = Color32::from_rgb(214, 74, 62);

/// A linear amplitude as decibels relative to full scale.
///
/// Silence, and anything quieter than [`MIN_DB`], comes back as [`MIN_DB`]
/// rather than as minus infinity, so the value is always usable as a position
/// on the bar.
#[must_use]
pub fn amplitude_to_db(amplitude: f32) -> f32 {
    if !amplitude.is_finite() || amplitude <= 0.0 {
        return MIN_DB;
    }
    (20.0 * amplitude.log10()).max(MIN_DB)
}

/// Where a level in decibels sits along the bar, in `0.0..=1.0`.
#[must_use]
pub fn db_fraction(decibels: f32) -> f32 {
    if !decibels.is_finite() {
        return 0.0;
    }
    ((decibels - MIN_DB) / -MIN_DB).clamp(0.0, 1.0)
}

/// Where a linear amplitude sits along the bar, in `0.0..=1.0`.
#[must_use]
pub fn amplitude_fraction(amplitude: f32) -> f32 {
    db_fraction(amplitude_to_db(amplitude))
}

/// One meter's state: the level it is showing, the peak it is holding and
/// whether it has seen a clipped block lately.
///
/// It is `Copy` and holds four floats, so a panel can keep one per track
/// without thinking about it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeterState {
    /// The levels of the most recent measurement.
    levels: MeterLevels,
    /// The peak being held, in decibels.
    hold_db: f32,
    /// Seconds left before the hold starts to fall.
    hold_left: f32,
    /// Seconds left on the clip indicator.
    clip_left: f32,
}

impl Default for MeterState {
    /// A silent meter with nothing held.
    fn default() -> Self {
        Self {
            levels: MeterLevels::SILENT,
            hold_db: MIN_DB,
            hold_left: 0.0,
            clip_left: 0.0,
        }
    }
}

impl MeterState {
    /// A silent meter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes one measurement, `elapsed` seconds after the last one.
    ///
    /// A louder peak than the one being held replaces it and restarts the
    /// hold; otherwise the hold sits for [`PEAK_HOLD_SECONDS`] and then falls
    /// at [`PEAK_FALL_DB_PER_SECOND`]. A clipped block lights the indicator
    /// for [`CLIP_HOLD_SECONDS`], and every further clipped block renews it.
    ///
    /// A negative or non-finite `elapsed` is treated as no time at all, so a
    /// clock that jumps backwards cannot make the hold rise.
    pub fn update(&mut self, levels: MeterLevels, elapsed: f32) {
        let elapsed = if elapsed.is_finite() {
            elapsed.max(0.0)
        } else {
            0.0
        };
        self.levels = MeterLevels {
            peak: sane(levels.peak),
            rms: sane(levels.rms),
        };
        let peak_db = amplitude_to_db(self.levels.peak);
        if peak_db >= self.hold_db {
            self.hold_db = peak_db;
            self.hold_left = PEAK_HOLD_SECONDS;
        } else if self.hold_left > elapsed {
            self.hold_left -= elapsed;
        } else {
            let falling = elapsed - self.hold_left;
            self.hold_left = 0.0;
            self.hold_db =
                (self.hold_db - falling * PEAK_FALL_DB_PER_SECOND).max(peak_db.max(MIN_DB));
        }
        if self.levels.is_clipping() {
            self.clip_left = CLIP_HOLD_SECONDS;
        } else {
            self.clip_left = (self.clip_left - elapsed).max(0.0);
        }
    }

    /// Lets the meter fall by `elapsed` seconds without a new measurement,
    /// which is what a stopped transport does.
    pub fn decay(&mut self, elapsed: f32) {
        self.update(MeterLevels::SILENT, elapsed);
    }

    /// Silences the meter and drops the hold and the clip indicator.
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// The most recent measurement.
    #[must_use]
    pub fn levels(&self) -> MeterLevels {
        self.levels
    }

    /// The peak being held, in decibels.
    #[must_use]
    pub fn peak_hold_db(&self) -> f32 {
        self.hold_db
    }

    /// Whether the clip indicator is lit.
    #[must_use]
    pub fn clipping(&self) -> bool {
        self.clip_left > 0.0
    }

    /// How much of the bar the RMS fills, in `0.0..=1.0`.
    #[must_use]
    pub fn rms_fraction(&self) -> f32 {
        amplitude_fraction(self.levels.rms)
    }

    /// Where the peak hold sits along the bar, in `0.0..=1.0`.
    #[must_use]
    pub fn hold_fraction(&self) -> f32 {
        db_fraction(self.hold_db)
    }

    /// Paints the meter into `rect`: the bar, the peak-hold tick, and the clip
    /// indicator at the loud end.
    ///
    /// The meter is drawn rather than laid out as a widget so that a track
    /// header can place it in a rectangle it already computed.
    pub fn paint(&self, painter: &Painter, rect: Rect, visuals: &Visuals) {
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return;
        }
        painter.rect_filled(rect, 1.0, visuals.extreme_bg_color);
        let clip_width = CLIP_WIDTH.min(rect.width() / 4.0);
        let bar = Rect::from_min_max(rect.min, pos2(rect.right() - clip_width, rect.bottom()));
        if bar.width() > 0.0 {
            let filled = bar.width() * self.rms_fraction();
            if filled > 0.0 {
                painter.rect_filled(
                    Rect::from_min_size(bar.min, vec2(filled, bar.height())),
                    1.0,
                    self.bar_color(),
                );
            }
            let hold = self.hold_fraction();
            if hold > 0.0 {
                let x = bar.left() + (bar.width() * hold).min(bar.width() - 1.0).max(0.0);
                painter.line_segment(
                    [pos2(x, bar.top()), pos2(x, bar.bottom())],
                    Stroke::new(1.0, visuals.strong_text_color()),
                );
            }
        }
        let indicator = Rect::from_min_max(pos2(bar.right(), rect.top()), rect.max);
        painter.rect_filled(
            indicator,
            1.0,
            if self.clipping() {
                CLIP_COLOR
            } else {
                visuals.faint_bg_color
            },
        );
    }

    /// Draws the meter into the space `ui` has left, at `height` points.
    ///
    /// This is the form the viewer uses, where the meter is one more thing in
    /// a row rather than a rectangle someone else measured.
    pub fn ui(&self, ui: &mut Ui, width: f32, height: f32) -> Rect {
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), eframe::egui::Sense::hover());
        self.paint(ui.painter(), rect, ui.visuals());
        rect
    }

    /// The colour of the bar at the level being held.
    fn bar_color(&self) -> Color32 {
        if self.clipping() {
            CLIP_COLOR
        } else if self.hold_db >= WARN_DB {
            WARN_COLOR
        } else {
            NORMAL_COLOR
        }
    }
}

/// A level with anything unusable — a NaN, an infinity, a negative — read as
/// silence, so a meter can never be asked to draw a bar of unknown length.
fn sane(level: f32) -> f32 {
    if level.is_finite() && level > 0.0 {
        level
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CLIP_HOLD_SECONDS, MIN_DB, MeterState, PEAK_FALL_DB_PER_SECOND, PEAK_HOLD_SECONDS,
        amplitude_fraction, amplitude_to_db, db_fraction,
    };
    use sub_audio::{MeterBank, MeterLevels};

    /// A frame at 60 Hz.
    const FRAME: f32 = 1.0 / 60.0;

    /// Asserts two levels match to within a step no meter could draw.
    fn close(left: f32, right: f32) {
        assert!((left - right).abs() < 1e-6, "expected {right}, got {left}");
    }

    #[test]
    fn silence_and_full_scale_sit_at_the_ends_of_the_bar() {
        assert!((amplitude_to_db(1.0) - 0.0).abs() < 1e-6);
        close(amplitude_to_db(0.0), MIN_DB);
        close(amplitude_to_db(-1.0), MIN_DB);
        close(amplitude_to_db(f32::NAN), MIN_DB);
        assert!((amplitude_fraction(1.0) - 1.0).abs() < 1e-6);
        close(amplitude_fraction(0.0), 0.0);
    }

    #[test]
    fn half_amplitude_is_about_six_decibels_down() {
        let db = amplitude_to_db(0.5);
        assert!((db + 6.0206).abs() < 1e-3, "half scale was {db} dB");
    }

    #[test]
    fn the_fraction_is_clamped_to_the_bar() {
        close(db_fraction(6.0), 1.0);
        close(db_fraction(-120.0), 0.0);
        assert!((db_fraction(MIN_DB / 2.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_new_meter_is_silent_with_nothing_held() {
        let meter = MeterState::new();
        assert_eq!(meter.levels(), MeterLevels::SILENT);
        close(meter.peak_hold_db(), MIN_DB);
        close(meter.hold_fraction(), 0.0);
        assert!(!meter.clipping());
    }

    #[test]
    fn the_bar_follows_the_measurement_at_once() {
        let mut meter = MeterState::new();
        meter.update(MeterLevels::new(0.5, 0.5), FRAME);
        assert!((meter.rms_fraction() - amplitude_fraction(0.5)).abs() < 1e-6);
        meter.update(MeterLevels::SILENT, FRAME);
        close(meter.rms_fraction(), 0.0);
    }

    #[test]
    fn the_peak_hold_sits_still_and_then_falls_at_the_stated_rate() {
        let mut meter = MeterState::new();
        meter.update(MeterLevels::new(1.0, 0.7), FRAME);
        assert!((meter.peak_hold_db() - 0.0).abs() < 1e-6);

        // Inside the hold window it does not move at all.
        meter.decay(PEAK_HOLD_SECONDS - 0.1);
        assert!((meter.peak_hold_db() - 0.0).abs() < 1e-6, "held early");

        // A second past the hold is one fall's worth of decibels.
        meter.decay(0.1 + 1.0);
        let expected = -PEAK_FALL_DB_PER_SECOND;
        assert!(
            (meter.peak_hold_db() - expected).abs() < 1e-3,
            "hold was {} dB, expected {expected}",
            meter.peak_hold_db()
        );
    }

    #[test]
    fn the_hold_never_falls_below_the_level_being_shown() {
        let mut meter = MeterState::new();
        meter.update(MeterLevels::new(1.0, 1.0), FRAME);
        for _ in 0..600 {
            meter.update(MeterLevels::new(0.5, 0.5), FRAME);
        }
        let floor = amplitude_to_db(0.5);
        assert!(
            (meter.peak_hold_db() - floor).abs() < 1e-3,
            "the hold settled at {} dB",
            meter.peak_hold_db()
        );
    }

    #[test]
    fn the_hold_bottoms_out_at_the_quietest_level_drawn() {
        let mut meter = MeterState::new();
        meter.update(MeterLevels::new(1.0, 1.0), FRAME);
        meter.decay(60.0);
        close(meter.peak_hold_db(), MIN_DB);
    }

    #[test]
    fn a_louder_peak_takes_the_hold_immediately() {
        let mut meter = MeterState::new();
        meter.update(MeterLevels::new(0.1, 0.1), FRAME);
        meter.update(MeterLevels::new(0.9, 0.9), FRAME);
        assert!((meter.peak_hold_db() - amplitude_to_db(0.9)).abs() < 1e-6);
    }

    #[test]
    fn one_clipped_block_latches_the_indicator_and_then_clears() {
        let mut meter = MeterState::new();
        meter.update(MeterLevels::new(1.0, 0.5), FRAME);
        assert!(meter.clipping());
        meter.decay(CLIP_HOLD_SECONDS - 0.01);
        assert!(meter.clipping(), "the indicator holds for its full time");
        meter.decay(0.02);
        assert!(!meter.clipping());
    }

    #[test]
    fn a_further_clip_renews_the_indicator() {
        let mut meter = MeterState::new();
        meter.update(MeterLevels::new(1.5, 0.5), FRAME);
        meter.decay(CLIP_HOLD_SECONDS - 0.1);
        meter.update(MeterLevels::new(1.5, 0.5), FRAME);
        meter.decay(CLIP_HOLD_SECONDS - 0.1);
        assert!(meter.clipping(), "the second clip restarted the hold");
    }

    #[test]
    fn clearing_drops_everything() {
        let mut meter = MeterState::new();
        meter.update(MeterLevels::new(1.2, 0.9), FRAME);
        meter.clear();
        assert_eq!(meter, MeterState::new());
    }

    #[test]
    fn nonsense_levels_read_as_silence() {
        let mut meter = MeterState::new();
        meter.update(MeterLevels::new(f32::NAN, f32::INFINITY), FRAME);
        assert_eq!(meter.levels(), MeterLevels::SILENT);
        assert!(!meter.clipping());
        close(meter.rms_fraction(), 0.0);
    }

    #[test]
    fn a_clock_that_goes_backwards_cannot_raise_the_hold() {
        let mut meter = MeterState::new();
        meter.update(MeterLevels::new(0.5, 0.5), FRAME);
        let held = meter.peak_hold_db();
        meter.update(MeterLevels::SILENT, -5.0);
        meter.update(MeterLevels::SILENT, f32::NAN);
        assert!(meter.peak_hold_db() <= held);
    }

    #[test]
    fn updating_a_full_set_of_meters_costs_well_under_a_tenth_of_a_millisecond() {
        // TASK-52 acceptance criterion 3: the per-frame meter update must cost
        // under 0.1 ms. A sequence with 64 audio tracks plus the master is far
        // more than an editing session has, and the whole set is updated here
        // the way a frame updates it: read the level, fold it into the hold,
        // and ask for the two fractions the painter needs.
        let mut meters = [MeterState::new(); 65];
        // Read through a real bank, so the measurement covers the atomic
        // loads the UI thread actually makes as well as the state update.
        let bank = MeterBank::new(64);
        for index in 0..bank.track_capacity() {
            bank.publish_track(index, MeterLevels::new(0.8, 0.6));
        }
        bank.publish_master(MeterLevels::new(0.8, 0.6));
        // Warm the caches and the branch predictor so the measurement is of
        // steady-state cost rather than of the first frame ever drawn.
        let read = |index: usize| bank.track(index).unwrap_or_else(|| bank.master());
        for _ in 0..100 {
            for (index, meter) in meters.iter_mut().enumerate() {
                meter.update(read(index), FRAME);
            }
        }
        let rounds = 1_000;
        let start = std::time::Instant::now();
        let mut sink = 0.0f32;
        for _ in 0..rounds {
            for (index, meter) in meters.iter_mut().enumerate() {
                meter.update(read(index), FRAME);
                sink += meter.rms_fraction() + meter.hold_fraction();
            }
        }
        let per_frame = start.elapsed().as_secs_f64() / f64::from(rounds);
        assert!(sink > 0.0, "the work must not be optimised away");
        assert!(
            per_frame < 0.000_1,
            "a frame's meter update took {:.1} us, over the 100 us budget",
            per_frame * 1e6
        );
    }
}
