//! The host side of the `audio-effect` world: block shape validation and the
//! real-time budget that bypasses a plugin which cannot keep up.
//!
//! An audio effect plugin is handed one block of interleaved `f32` samples and
//! must hand back a block of the same shape. Two things can go wrong, and the
//! host treats them the same way: the plugin returns something malformed, or it
//! takes too long. [`RealTimeBudget`] counts both as faults, and after a run of
//! them it latches into bypass so playback continues with the plugin's input
//! passed through untouched.
//!
//! # Where this runs
//!
//! Not in the audio callback. Calling into WebAssembly allocates and can block,
//! and the callback may do neither (docs/PLAN.md §4), so the host processes
//! plugin blocks on an engine worker ahead of the callback. The budget itself
//! is still written to callback rules — every field is [`Copy`], no method
//! allocates, locks or panics — so the same accounting can sit directly beside
//! the mixer when a native effect tier arrives.
//!
//! ```
//! use std::time::Duration;
//! use sub_plugin::audio::{BlockFormat, BlockOutcome, RealTimeBudget};
//!
//! // 512 frames of stereo at 48 kHz is 10.666 ms of audio; half of that is the
//! // plugin's share.
//! let format = BlockFormat::new(512, 2, 48_000).unwrap();
//! let mut budget = RealTimeBudget::for_format(format, 50, 2).unwrap();
//! assert_eq!(budget.budget(), Duration::from_nanos(5_333_333));
//!
//! // Blocks inside the budget just pass.
//! assert_eq!(budget.record(Duration::from_millis(1)), BlockOutcome::Within);
//!
//! // Three overruns in a row: two tolerated, the third latches bypass.
//! assert_eq!(budget.record(Duration::from_millis(9)), BlockOutcome::Overrun);
//! assert_eq!(budget.record(Duration::from_millis(9)), BlockOutcome::Overrun);
//! assert_eq!(budget.record(Duration::from_millis(9)), BlockOutcome::Bypassed);
//! assert!(budget.is_bypassed());
//! ```

use std::time::{Duration, Instant};

use sub_core::SubError;

use crate::codes;

/// Nanoseconds in one second, as the block arithmetic needs it.
const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// The shape of one block crossing into an `audio-effect` plugin.
///
/// Frames, channels and sample rate are all non-zero, so [`Self::wall_clock`]
/// and the length checks cannot divide by zero however a plugin or a project
/// file is configured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockFormat {
    /// Frames per channel in one block.
    frames: u32,
    /// Interleaved channels.
    channels: u32,
    /// Sample rate in hertz.
    sample_rate: u32,
}

impl BlockFormat {
    /// Builds a format, rejecting a zero in any field.
    ///
    /// # Errors
    ///
    /// [`codes::INVALID_BLOCK_FORMAT`] when `frames`, `channels` or
    /// `sample_rate` is zero.
    pub fn new(frames: u32, channels: u32, sample_rate: u32) -> Result<Self, SubError> {
        if frames == 0 || channels == 0 || sample_rate == 0 {
            return Err(SubError::new(
                codes::INVALID_BLOCK_FORMAT,
                "an audio block format needs non-zero frames, channels and sample rate",
            )
            .with_detail("frames", frames)
            .with_detail("channels", channels)
            .with_detail("sample_rate", sample_rate));
        }
        Ok(Self {
            frames,
            channels,
            sample_rate,
        })
    }

    /// Frames per channel in one block.
    pub fn frames(self) -> u32 {
        self.frames
    }

    /// Interleaved channels in one block.
    pub fn channels(self) -> u32 {
        self.channels
    }

    /// The sample rate in hertz.
    pub fn sample_rate(self) -> u32 {
        self.sample_rate
    }

    /// How many `f32` samples one interleaved block holds.
    pub fn samples(self) -> usize {
        self.frames as usize * self.channels as usize
    }

    /// How long the block lasts in real time: the wall clock the plugin is
    /// racing.
    ///
    /// Computed in whole nanoseconds from the exact frame count and rate; no
    /// float ever enters the arithmetic.
    pub fn wall_clock(self) -> Duration {
        let nanos = u64::from(self.frames) * NANOS_PER_SECOND / u64::from(self.sample_rate);
        Duration::from_nanos(nanos)
    }

    /// Checks a buffer the host is about to hand a plugin.
    ///
    /// # Errors
    ///
    /// [`codes::INVALID_AUDIO_BLOCK`] when the buffer is not exactly
    /// [`Self::samples`] long.
    pub fn validate(self, block: &[f32]) -> Result<(), SubError> {
        if block.len() == self.samples() {
            return Ok(());
        }
        Err(SubError::new(
            codes::INVALID_AUDIO_BLOCK,
            "an interleaved audio block must hold frames * channels samples",
        )
        .with_detail("expected", self.samples())
        .with_detail("actual", block.len())
        .with_detail("channels", self.channels))
    }
}

/// What the host does with one block after timing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockOutcome {
    /// The plugin finished inside its budget; use its output.
    Within,
    /// The plugin overran or faulted, but not often enough to be bypassed:
    /// drop this block's output and pass the input through.
    Overrun,
    /// The plugin is bypassed from now until the host resets the budget. Pass
    /// the input through and stop calling the plugin.
    Bypassed,
}

impl BlockOutcome {
    /// Whether the host may use the plugin's output for this block.
    pub fn output_is_usable(self) -> bool {
        matches!(self, Self::Within)
    }
}

/// A per-block time budget that latches into bypass after repeated overruns.
///
/// The budget is a fraction of the block's own wall-clock duration, so it
/// scales with the block size and sample rate the engine happens to be running:
/// a plugin that is fast enough at 512 frames is held to the same share of real
/// time at 64.
///
/// Faults are counted consecutively. A plugin that overruns once in a while
/// costs one block of audio each time; one that overruns
/// `tolerated_overruns + 1` times in a row is bypassed and is not called again
/// until [`Self::reset`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RealTimeBudget {
    /// The deadline for one block.
    budget: Duration,
    /// How many consecutive faults are forgiven before bypass.
    tolerated_overruns: u32,
    /// Consecutive faults so far.
    consecutive_overruns: u32,
    /// Blocks timed since the last reset.
    blocks: u64,
    /// Faults seen since the last reset.
    overruns: u64,
    /// Whether the plugin is bypassed.
    bypassed: bool,
}

impl RealTimeBudget {
    /// The share of real time the host gives a plugin when nothing else says
    /// otherwise: half of one block, leaving the other half for decode, the
    /// mixer and the callback itself.
    pub const DEFAULT_PERCENT: u32 = 50;

    /// How many consecutive faults the host forgives by default. Three lets a
    /// plugin survive a scheduler hiccup; a fourth in a row is a plugin that
    /// cannot keep up.
    pub const DEFAULT_TOLERATED_OVERRUNS: u32 = 3;

    /// Builds a budget from an explicit deadline.
    ///
    /// # Errors
    ///
    /// [`codes::INVALID_BUDGET`] when `budget` is zero, which would bypass
    /// every plugin on its first block.
    pub fn new(budget: Duration, tolerated_overruns: u32) -> Result<Self, SubError> {
        if budget.is_zero() {
            return Err(SubError::new(
                codes::INVALID_BUDGET,
                "a real-time budget must be longer than zero",
            )
            .with_detail("tolerated_overruns", tolerated_overruns));
        }
        Ok(Self {
            budget,
            tolerated_overruns,
            consecutive_overruns: 0,
            blocks: 0,
            overruns: 0,
            bypassed: false,
        })
    }

    /// Builds a budget as `percent` of one block's wall-clock duration.
    ///
    /// # Errors
    ///
    /// [`codes::INVALID_BUDGET`] when `percent` is zero or above 100: a plugin
    /// may not be given longer than real time, or no time at all.
    pub fn for_format(
        format: BlockFormat,
        percent: u32,
        tolerated_overruns: u32,
    ) -> Result<Self, SubError> {
        if percent == 0 || percent > 100 {
            return Err(SubError::new(
                codes::INVALID_BUDGET,
                "a real-time budget is between 1 and 100 percent of one block",
            )
            .with_detail("percent", percent));
        }
        let nanos = u64::from(format.frames) * NANOS_PER_SECOND * u64::from(percent)
            / (u64::from(format.sample_rate) * 100);
        Self::new(Duration::from_nanos(nanos.max(1)), tolerated_overruns)
    }

    /// Builds a budget from the host's policy and the plugin's own claim,
    /// taking whichever is smaller. A `claim` of zero means the plugin's
    /// `describe` made no claim, so the host's policy stands alone.
    ///
    /// # Errors
    ///
    /// As [`Self::for_format`], after the two percentages are combined; a claim
    /// above 100 is clamped to the host's policy rather than rejected, because
    /// it can only ever narrow the budget.
    pub fn for_claim(
        format: BlockFormat,
        host_percent: u32,
        claim: u32,
        tolerated_overruns: u32,
    ) -> Result<Self, SubError> {
        let percent = if claim == 0 || claim > 100 {
            host_percent
        } else {
            host_percent.min(claim)
        };
        Self::for_format(format, percent, tolerated_overruns)
    }

    /// The deadline one block must meet.
    pub fn budget(self) -> Duration {
        self.budget
    }

    /// How many consecutive faults are forgiven before bypass.
    pub fn tolerated_overruns(self) -> u32 {
        self.tolerated_overruns
    }

    /// Whether the plugin is bypassed.
    pub fn is_bypassed(self) -> bool {
        self.bypassed
    }

    /// Blocks timed since the last reset.
    pub fn blocks(self) -> u64 {
        self.blocks
    }

    /// Faults — overruns and plugin failures — since the last reset.
    pub fn overruns(self) -> u64 {
        self.overruns
    }

    /// Consecutive faults immediately behind the current block.
    pub fn consecutive_overruns(self) -> u32 {
        self.consecutive_overruns
    }

    /// Records how long one block actually took.
    ///
    /// Allocation-free and lock-free: the host may call it wherever it times
    /// the plugin.
    pub fn record(&mut self, elapsed: Duration) -> BlockOutcome {
        if self.bypassed {
            return BlockOutcome::Bypassed;
        }
        self.blocks += 1;
        if elapsed <= self.budget {
            self.consecutive_overruns = 0;
            return BlockOutcome::Within;
        }
        self.fault_locked()
    }

    /// Records a block the plugin failed outright: it trapped, returned an
    /// error, or returned a buffer of the wrong shape.
    ///
    /// A failure costs the plugin exactly what an overrun costs it, so a plugin
    /// that always errors is bypassed just as fast as one that is always late.
    pub fn fault(&mut self) -> BlockOutcome {
        if self.bypassed {
            return BlockOutcome::Bypassed;
        }
        self.blocks += 1;
        self.fault_locked()
    }

    /// The shared tail of [`Self::record`] and [`Self::fault`], once the block
    /// has been counted and bypass ruled out.
    fn fault_locked(&mut self) -> BlockOutcome {
        self.overruns += 1;
        self.consecutive_overruns += 1;
        if self.consecutive_overruns > self.tolerated_overruns {
            self.bypassed = true;
            return BlockOutcome::Bypassed;
        }
        BlockOutcome::Overrun
    }

    /// Starts timing one block, or returns `None` when the plugin is already
    /// bypassed and must not be called at all.
    pub fn start_block(&mut self) -> Option<BlockTimer<'_>> {
        if self.bypassed {
            return None;
        }
        Some(BlockTimer {
            budget: self,
            start: Instant::now(),
        })
    }

    /// Clears the bypass and every counter: the host calls this when the plugin
    /// is reloaded, re-enabled, or the block format changes.
    pub fn reset(&mut self) {
        self.consecutive_overruns = 0;
        self.blocks = 0;
        self.overruns = 0;
        self.bypassed = false;
    }

    /// The error to surface once the plugin has been bypassed, for the log and
    /// the inspector. Allocates, so it belongs off the processing path.
    pub fn bypass_error(self) -> Option<SubError> {
        if !self.bypassed {
            return None;
        }
        Some(
            SubError::new(
                codes::AUDIO_BYPASSED,
                "the audio effect was bypassed after repeatedly missing its real-time budget",
            )
            .with_detail("budget_nanos", self.budget.as_nanos())
            .with_detail("tolerated_overruns", self.tolerated_overruns)
            .with_detail("overruns", self.overruns)
            .with_detail("blocks", self.blocks),
        )
    }
}

/// One block being timed, handed out by [`RealTimeBudget::start_block`].
///
/// The timer borrows the budget, so the host cannot forget to close a block: it
/// calls [`Self::finish`] with the plugin's result or [`Self::fault`] when the
/// call failed, and either one returns what to do with the output.
#[derive(Debug)]
pub struct BlockTimer<'a> {
    /// The budget this block is charged against.
    budget: &'a mut RealTimeBudget,
    /// When the block started.
    start: Instant,
}

impl BlockTimer<'_> {
    /// How long the block has been running so far.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Closes a block the plugin returned from, charging it its elapsed time.
    pub fn finish(self) -> BlockOutcome {
        let elapsed = self.start.elapsed();
        self.budget.record(elapsed)
    }

    /// Closes a block the plugin failed: a trap, an error, or a malformed
    /// output buffer.
    pub fn fault(self) -> BlockOutcome {
        self.budget.fault()
    }
}

/// Runs one block through a plugin under its budget, and says what the host
/// should do with the result.
///
/// `call` is the guest call — [`crate::AudioEffect`]'s `process` once TASK-84
/// owns instances, and a closure in tests. Everything a plugin can get wrong is
/// funnelled into one place here: it can be slow, it can return an error, and
/// it can return a buffer that is not the block it was given. All three charge
/// the same fault, so a plugin that misbehaves in any of those ways for
/// `tolerated_overruns + 1` blocks in a row ends up bypassed.
///
/// `Some(output)` is a buffer the host may use; `None` means pass the input
/// through untouched for this block.
pub fn process_block<F>(
    budget: &mut RealTimeBudget,
    format: BlockFormat,
    call: F,
) -> (BlockOutcome, Option<Vec<f32>>)
where
    F: FnOnce() -> Result<Vec<f32>, SubError>,
{
    let Some(timer) = budget.start_block() else {
        return (BlockOutcome::Bypassed, None);
    };
    match call() {
        // A malformed buffer is charged as a fault whether or not it was fast:
        // the host cannot use it either way.
        Ok(output) if format.validate(&output).is_err() => (timer.fault(), None),
        Ok(output) => {
            let outcome = timer.finish();
            if outcome.output_is_usable() {
                (outcome, Some(output))
            } else {
                (outcome, None)
            }
        }
        Err(_) => (timer.fault(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockFormat, BlockOutcome, RealTimeBudget, process_block};
    use crate::codes;
    use std::time::Duration;
    use sub_core::{ErrorCode, SubError};

    /// A stereo 48 kHz block of 512 frames: the common engine case.
    fn format() -> BlockFormat {
        BlockFormat::new(512, 2, 48_000).unwrap()
    }

    #[test]
    fn a_block_format_rejects_zero_fields() {
        for (frames, channels, rate) in [(0, 2, 48_000), (512, 0, 48_000), (512, 2, 0)] {
            let error = BlockFormat::new(frames, channels, rate).unwrap_err();
            assert_eq!(error.code, codes::INVALID_BLOCK_FORMAT);
        }
    }

    #[test]
    fn wall_clock_is_exact_integer_nanoseconds() {
        assert_eq!(format().wall_clock(), Duration::from_nanos(10_666_666));
        assert_eq!(
            BlockFormat::new(48_000, 1, 48_000).unwrap().wall_clock(),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn a_block_must_hold_frames_times_channels_samples() {
        let format = format();
        assert_eq!(format.samples(), 1_024);
        format.validate(&vec![0.0; 1_024]).unwrap();
        let error = format.validate(&vec![0.0; 1_023]).unwrap_err();
        assert_eq!(error.code, codes::INVALID_AUDIO_BLOCK);
    }

    #[test]
    fn the_budget_is_a_share_of_the_blocks_own_duration() {
        let half = RealTimeBudget::for_format(format(), 50, 3).unwrap();
        assert_eq!(half.budget(), Duration::from_nanos(5_333_333));
        let all = RealTimeBudget::for_format(format(), 100, 3).unwrap();
        assert_eq!(all.budget(), Duration::from_nanos(10_666_666));
        // A smaller block at the same rate gets proportionally less time.
        let small = BlockFormat::new(64, 2, 48_000).unwrap();
        assert_eq!(
            RealTimeBudget::for_format(small, 50, 3).unwrap().budget(),
            Duration::from_nanos(666_666)
        );
    }

    #[test]
    fn a_budget_outside_one_to_one_hundred_percent_is_rejected() {
        for percent in [0, 101, u32::MAX] {
            let error = RealTimeBudget::for_format(format(), percent, 3).unwrap_err();
            assert_eq!(error.code, codes::INVALID_BUDGET);
        }
        assert_eq!(
            RealTimeBudget::new(Duration::ZERO, 3).unwrap_err().code,
            codes::INVALID_BUDGET
        );
    }

    #[test]
    fn a_plugins_claim_can_only_narrow_the_hosts_policy() {
        let host_only = RealTimeBudget::for_format(format(), 50, 3)
            .unwrap()
            .budget();
        // No claim, or an absurd one, leaves the host's policy standing.
        assert_eq!(
            RealTimeBudget::for_claim(format(), 50, 0, 3)
                .unwrap()
                .budget(),
            host_only
        );
        assert_eq!(
            RealTimeBudget::for_claim(format(), 50, 400, 3)
                .unwrap()
                .budget(),
            host_only
        );
        // A greedier claim than the host allows is capped.
        assert_eq!(
            RealTimeBudget::for_claim(format(), 50, 90, 3)
                .unwrap()
                .budget(),
            host_only
        );
        // A modest claim is honoured.
        assert_eq!(
            RealTimeBudget::for_claim(format(), 50, 10, 3)
                .unwrap()
                .budget(),
            RealTimeBudget::for_format(format(), 10, 3)
                .unwrap()
                .budget()
        );
    }

    #[test]
    fn blocks_inside_the_budget_never_bypass() {
        let mut budget = RealTimeBudget::new(Duration::from_millis(5), 0).unwrap();
        for _ in 0..1_000 {
            assert_eq!(
                budget.record(Duration::from_millis(5)),
                BlockOutcome::Within
            );
        }
        assert!(!budget.is_bypassed());
        assert_eq!(budget.blocks(), 1_000);
        assert_eq!(budget.overruns(), 0);
        assert_eq!(budget.bypass_error(), None);
    }

    #[test]
    fn scattered_overruns_cost_a_block_each_but_do_not_bypass() {
        let mut budget = RealTimeBudget::new(Duration::from_millis(5), 2).unwrap();
        for _ in 0..50 {
            assert_eq!(
                budget.record(Duration::from_millis(9)),
                BlockOutcome::Overrun
            );
            assert_eq!(
                budget.record(Duration::from_millis(9)),
                BlockOutcome::Overrun
            );
            assert_eq!(
                budget.record(Duration::from_millis(1)),
                BlockOutcome::Within
            );
        }
        assert!(!budget.is_bypassed());
        assert_eq!(budget.overruns(), 100);
        assert_eq!(budget.consecutive_overruns(), 0);
    }

    #[test]
    fn repeated_overruns_latch_the_plugin_into_bypass() {
        let mut budget = RealTimeBudget::new(Duration::from_millis(5), 2).unwrap();
        assert_eq!(
            budget.record(Duration::from_millis(6)),
            BlockOutcome::Overrun
        );
        assert_eq!(
            budget.record(Duration::from_millis(6)),
            BlockOutcome::Overrun
        );
        assert_eq!(
            budget.record(Duration::from_millis(6)),
            BlockOutcome::Bypassed
        );
        assert!(budget.is_bypassed());

        // Bypass is latched: a fast block does not un-bypass the plugin, and
        // bypassed blocks are not counted, because they never ran.
        let blocks = budget.blocks();
        assert_eq!(
            budget.record(Duration::from_nanos(1)),
            BlockOutcome::Bypassed
        );
        assert_eq!(budget.blocks(), blocks);
        assert!(budget.start_block().is_none());

        let error = budget.bypass_error().unwrap();
        assert_eq!(error.code, codes::AUDIO_BYPASSED);
        assert_eq!(error.details["overruns"], 3);

        budget.reset();
        assert!(!budget.is_bypassed());
        assert_eq!(budget.blocks(), 0);
        assert_eq!(budget.overruns(), 0);
        assert!(budget.start_block().is_some());
    }

    #[test]
    fn a_failing_plugin_is_bypassed_as_fast_as_a_slow_one() {
        let mut budget = RealTimeBudget::new(Duration::from_millis(5), 1).unwrap();
        assert_eq!(budget.fault(), BlockOutcome::Overrun);
        assert_eq!(budget.fault(), BlockOutcome::Bypassed);
        assert!(budget.is_bypassed());
        assert_eq!(budget.fault(), BlockOutcome::Bypassed);
    }

    #[test]
    fn only_a_block_inside_the_budget_yields_usable_output() {
        assert!(BlockOutcome::Within.output_is_usable());
        assert!(!BlockOutcome::Overrun.output_is_usable());
        assert!(!BlockOutcome::Bypassed.output_is_usable());
    }

    #[test]
    fn a_fast_well_behaved_plugin_keeps_its_output() {
        let format = format();
        let mut budget = RealTimeBudget::for_format(format, 50, 3).unwrap();
        for _ in 0..100 {
            let (outcome, output) =
                process_block(&mut budget, format, || Ok(vec![0.5; format.samples()]));
            assert_eq!(outcome, BlockOutcome::Within);
            assert_eq!(output.unwrap().len(), format.samples());
        }
        assert!(!budget.is_bypassed());
        assert_eq!(budget.overruns(), 0);
    }

    #[test]
    fn a_slow_plugin_is_bypassed_and_stops_being_called() {
        let format = format();
        // One millisecond of budget against a plugin that sleeps for twenty.
        let mut budget = RealTimeBudget::new(Duration::from_millis(1), 1).unwrap();
        let mut calls = 0_u32;
        let mut slow = |budget: &mut RealTimeBudget| {
            process_block(budget, format, || {
                calls += 1;
                std::thread::sleep(Duration::from_millis(20));
                Ok(vec![0.5; format.samples()])
            })
        };

        assert_eq!(slow(&mut budget).0, BlockOutcome::Overrun);
        assert_eq!(slow(&mut budget).0, BlockOutcome::Bypassed);
        // Bypassed: the plugin is not entered again, and the host passes the
        // input through.
        let (outcome, output) = slow(&mut budget);
        assert_eq!(outcome, BlockOutcome::Bypassed);
        assert!(output.is_none());
        assert_eq!(calls, 2);
    }

    #[test]
    fn a_plugin_that_errors_or_returns_the_wrong_shape_is_bypassed_too() {
        let format = format();
        let mut budget = RealTimeBudget::for_format(format, 100, 2).unwrap();

        // A returned error costs a fault.
        let (outcome, output) = process_block(&mut budget, format, || {
            Err(SubError::new(
                ErrorCode::from_static("plugin.trapped"),
                "boom",
            ))
        });
        assert_eq!(outcome, BlockOutcome::Overrun);
        assert!(output.is_none());

        // So does a buffer that is not the block it was handed, however fast it
        // came back.
        let (outcome, output) = process_block(&mut budget, format, || Ok(vec![0.5; 7]));
        assert_eq!(outcome, BlockOutcome::Overrun);
        assert!(output.is_none());

        let (outcome, _) = process_block(&mut budget, format, || Ok(Vec::new()));
        assert_eq!(outcome, BlockOutcome::Bypassed);
        assert!(budget.is_bypassed());
        assert_eq!(budget.overruns(), 3);
    }

    #[test]
    fn a_timer_charges_the_block_it_borrows() {
        let mut budget = RealTimeBudget::new(Duration::from_mins(1), 0).unwrap();
        let timer = budget.start_block().unwrap();
        assert!(timer.elapsed() < Duration::from_mins(1));
        assert_eq!(timer.finish(), BlockOutcome::Within);
        assert_eq!(budget.blocks(), 1);

        // A zero-length budget cannot be built, so a fault is the only way to
        // drive a timer to bypass deterministically.
        let timer = budget.start_block().unwrap();
        assert_eq!(timer.fault(), BlockOutcome::Bypassed);
        assert!(budget.is_bypassed());
    }
}
