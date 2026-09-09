//! The lock-free mixer graph: clips into tracks into the master bus
//! (docs/PLAN.md §5.4).
//!
//! The graph has three levels. Every clip owns a ring buffer that a worker
//! thread fills with frames already resampled to the sequence rate (see
//! [`crate::resample`]); the mixer pulls from it, applies the clip gain and
//! its fade in and fade out, and sums the result into the clip's track. A
//! track applies its own gain and drops out when it is muted or when another
//! track is soloed. The tracks sum into the master bus, which applies the
//! master gain and mute.
//!
//! Two halves, one thread each:
//!
//! - The **engine thread** owns a [`MixerControl`]. It builds an immutable
//!   [`MixGraph`] with [`MixGraphBuilder`] — all timeline positions given as
//!   [`RationalTime`] and converted to exact frame counts there, never in the
//!   callback — and publishes it.
//! - The **audio callback** owns a [`Mixer`] and calls [`Mixer::process`].
//!
//! Nothing crosses between them but a lock-free single-producer
//! single-consumer ring. A published graph is swapped in atomically at the top
//! of a callback: the callback takes the new [`Arc`] and hands the one it
//! displaced back over a second ring, so the audio thread never drops the last
//! reference to a graph and never frees a ring buffer. The callback allocates
//! nothing, locks nothing and never blocks; a clip whose ring has run dry
//! contributes silence and bumps [`Mixer::underrun_frames`].
//!
//! ```
//! # fn main() -> sub_core::SubResult<()> {
//! use sub_audio::mixer::{ClipSpec, MixGraphBuilder, MixerConfig, TrackSpec, mixer};
//! use sub_audio::resample::pcm_ring;
//! use sub_time::{Rational, RationalTime};
//!
//! let rate = Rational::from_integer(48_000).expect("a valid rate");
//! let graph = MixGraphBuilder::new(48_000, 2)
//!     .track(TrackSpec::new().with_gain_db(-3.0).with_clip(
//!         ClipSpec::new(0, RationalTime::zero(rate), RationalTime::new(48_000, rate))
//!             .with_fades(RationalTime::new(2_400, rate), RationalTime::new(2_400, rate)),
//!     ))
//!     .build()?;
//!
//! let (mut control, mut mixer) = mixer(graph, MixerConfig::default())?;
//!
//! // Engine thread: hand the callback the reading end of the clip's ring.
//! let (mut writer, reader) = pcm_ring(2, 4_096)?;
//! writer.write(&[0.5; 2 * 1_024]);
//! control.install_slot(0, reader)?;
//!
//! // Audio callback: no lock, no allocation.
//! let mut block = [0.0f32; 2 * 256];
//! let frames = mixer.process(&mut block);
//! assert_eq!(frames, 256);
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;

use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer, Split};

use sub_core::{SubError, SubResult};
use sub_time::{Rational, RationalTime, Rounding};

use crate::codes;
use crate::decode::MAX_CHANNELS;
use crate::meter::{MeterBank, levels_of};
use crate::resample::PcmReader;

/// The lowest gain the mixer accepts, in decibels. A bus at this level is
/// silent, matching `sub_model::GainDb::SILENT`.
pub const MIN_GAIN_DB: f64 = -144.0;

/// The highest gain the mixer accepts, in decibels, matching
/// `sub_model::GainDb::MAX`.
pub const MAX_GAIN_DB: f64 = 24.0;

/// Converts a level in decibels to the linear factor the mixer multiplies by.
///
/// # Errors
///
/// Returns [`codes::INVALID_GAIN`] when the level is not a finite number
/// within [`MIN_GAIN_DB`]`..=`[`MAX_GAIN_DB`].
#[allow(
    clippy::cast_possible_truncation,
    reason = "audio is mixed in f32; the exponent is far inside its range"
)]
pub fn linear_gain(decibels: f64) -> SubResult<f32> {
    if !decibels.is_finite() || !(MIN_GAIN_DB..=MAX_GAIN_DB).contains(&decibels) {
        return Err(SubError::new(
            codes::INVALID_GAIN,
            "gain must be a finite level between -144 dB and +24 dB inclusive",
        )
        .with_detail("decibels", decibels.to_string()));
    }
    if decibels <= MIN_GAIN_DB {
        return Ok(0.0);
    }
    Ok(10.0f64.powf(decibels / 20.0) as f32)
}

/// The sequence timebase for `sample_rate`, as a rational.
fn sample_rate_of(sample_rate: u32) -> SubResult<Rational> {
    Rational::from_integer(sample_rate).ok_or_else(|| {
        SubError::new(
            sub_core::codes::INVALID_ARGUMENT,
            "a sequence sample rate must be greater than zero",
        )
        .with_detail("sample_rate", sample_rate)
    })
}

/// A non-negative duration or position in whole audio frames at `rate`.
///
/// This is the only place a timeline value meets the sample clock: exact where
/// the rates divide, rounded to the nearest frame where they do not, since a
/// 1001-denominator rate never lands on a whole sample.
pub(crate) fn frames_at(time: RationalTime, rate: Rational, what: &'static str) -> SubResult<u64> {
    if time.is_negative() {
        return Err(graph_invalid(what, "must not be negative", time));
    }
    let scaled = time
        .checked_rescaled_to_rounding(rate, Rounding::Nearest)
        .ok_or_else(|| graph_invalid(what, "is not representable at the sample rate", time))?;
    u64::try_from(scaled.value())
        .map_err(|_| graph_invalid(what, "is longer than the mixer can count", time))
}

/// A [`codes::GRAPH_INVALID`] error naming the offending field and value.
fn graph_invalid(what: &'static str, why: &str, time: RationalTime) -> SubError {
    SubError::new(codes::GRAPH_INVALID, format!("{what} {why}"))
        .with_detail("field", what)
        .with_detail("value", time.to_string())
}

/// One clip on a track, as the engine describes it.
///
/// Times are sequence times: `start` is where the clip begins on the timeline
/// and `duration` how long it plays there. `slot` is the index of the ring
/// buffer the clip's frames arrive on, installed with
/// [`MixerControl::install_slot`]; the frames in that ring begin at the clip's
/// own first frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipSpec {
    /// Index of the clip's ring buffer.
    pub slot: usize,
    /// Where the clip starts in sequence time.
    pub start: RationalTime,
    /// How long the clip plays.
    pub duration: RationalTime,
    /// Length of the fade up from silence at the clip's head.
    pub fade_in: RationalTime,
    /// Length of the fade down to silence at the clip's tail.
    pub fade_out: RationalTime,
    /// The clip's own level, in decibels.
    pub gain_db: f64,
}

impl ClipSpec {
    /// A clip at unity gain with no fades.
    #[must_use]
    pub fn new(slot: usize, start: RationalTime, duration: RationalTime) -> Self {
        let zero = RationalTime::zero(start.rate());
        Self {
            slot,
            start,
            duration,
            fade_in: zero,
            fade_out: zero,
            gain_db: 0.0,
        }
    }

    /// The same clip at `gain_db` decibels.
    #[must_use]
    pub fn with_gain_db(mut self, gain_db: f64) -> Self {
        self.gain_db = gain_db;
        self
    }

    /// The same clip with the two fade lengths.
    #[must_use]
    pub fn with_fades(mut self, fade_in: RationalTime, fade_out: RationalTime) -> Self {
        self.fade_in = fade_in;
        self.fade_out = fade_out;
        self
    }
}

/// One audio track, as the engine describes it.
///
/// A muted track, and any track that is not soloed while some other track is,
/// contributes nothing to the master. Its clips are still pulled from their
/// rings so that they stay aligned with the transport.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackSpec {
    /// The track fader, in decibels.
    pub gain_db: f64,
    /// Whether the track is silenced.
    pub muted: bool,
    /// Whether the track is soloed.
    pub solo: bool,
    /// The clips laid out along the track.
    pub clips: Vec<ClipSpec>,
}

impl TrackSpec {
    /// An empty track at unity gain, neither muted nor soloed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The same track at `gain_db` decibels.
    #[must_use]
    pub fn with_gain_db(mut self, gain_db: f64) -> Self {
        self.gain_db = gain_db;
        self
    }

    /// The same track, muted or not.
    #[must_use]
    pub fn with_muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }

    /// The same track, soloed or not.
    #[must_use]
    pub fn with_solo(mut self, solo: bool) -> Self {
        self.solo = solo;
        self
    }

    /// The same track with one more clip on it.
    #[must_use]
    pub fn with_clip(mut self, clip: ClipSpec) -> Self {
        self.clips.push(clip);
        self
    }
}

/// A clip compiled for the callback: frame counts and linear gains only.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ClipNode {
    /// Index of the clip's ring buffer.
    slot: usize,
    /// First frame of the clip, counted from sequence zero.
    start: u64,
    /// How many frames the clip plays.
    frames: u64,
    /// Frames of fade in at the head.
    fade_in: u64,
    /// Frames of fade out at the tail.
    fade_out: u64,
    /// The clip's linear gain.
    gain: f32,
}

impl ClipNode {
    /// One past the clip's last frame.
    fn end(self) -> u64 {
        self.start + self.frames
    }

    /// How many frames into the clip the sequence frame `at` falls.
    fn offset_of(self, at: u64) -> u64 {
        at.saturating_sub(self.start)
    }

    /// The fade envelope at `offset` frames into the clip, in `0.0..=1.0`.
    ///
    /// Both fades are linear in amplitude: the fade in is silent on the clip's
    /// first frame and reaches unity `fade_in` frames later; the fade out
    /// leaves unity `fade_out` frames before the end and would reach silence
    /// one frame past it.
    #[allow(
        clippy::cast_precision_loss,
        reason = "fade lengths are far below f32's exact integer range"
    )]
    fn envelope(self, offset: u64) -> f32 {
        let mut factor = 1.0f32;
        if self.fade_in > 0 && offset < self.fade_in {
            factor *= offset as f32 / self.fade_in as f32;
        }
        let remaining = self.frames.saturating_sub(offset);
        if self.fade_out > 0 && remaining <= self.fade_out {
            factor *= remaining as f32 / self.fade_out as f32;
        }
        factor
    }
}

/// A track compiled for the callback.
#[derive(Debug, Clone, PartialEq)]
struct TrackNode {
    /// The track's linear fader.
    gain: f32,
    /// Whether the track reaches the master at all.
    audible: bool,
    /// The clips on the track, in the order the engine listed them.
    clips: Vec<ClipNode>,
}

/// An immutable description of the whole mixer.
///
/// Built on the engine thread by [`MixGraphBuilder`], published with
/// [`MixerControl::publish`] and never mutated afterwards: the callback only
/// reads it, and a change is a whole new graph swapped in.
#[derive(Debug, Clone, PartialEq)]
pub struct MixGraph {
    /// The sequence sample rate every clip's ring already runs at.
    sample_rate: u32,
    /// Channels per frame on every bus.
    channels: u16,
    /// How many ring buffer slots the clips refer to.
    slot_count: usize,
    /// The master fader, linear.
    master_gain: f32,
    /// Whether the master is muted.
    master_muted: bool,
    /// Whether any track is soloed, which silences the tracks that are not.
    any_solo: bool,
    /// The tracks, in the order the engine listed them.
    tracks: Vec<TrackNode>,
}

impl MixGraph {
    /// The sequence sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The sequence sample rate as a timebase, for positions in frames.
    ///
    /// # Panics
    ///
    /// Never: the rate was validated when the graph was built.
    pub fn rate(&self) -> Rational {
        Rational::from_integer(self.sample_rate).expect("a graph holds a validated sample rate")
    }

    /// Channels per frame.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// How many ring buffer slots the graph's clips refer to.
    pub fn slot_count(&self) -> usize {
        self.slot_count
    }

    /// How many tracks the graph has.
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// How many clips the graph has in total.
    pub fn clip_count(&self) -> usize {
        self.tracks.iter().map(|track| track.clips.len()).sum()
    }

    /// Whether some track is soloed.
    pub fn any_solo(&self) -> bool {
        self.any_solo
    }

    /// The master fader as a linear factor.
    pub fn master_gain(&self) -> f32 {
        self.master_gain
    }

    /// Whether the master bus is muted.
    pub fn master_muted(&self) -> bool {
        self.master_muted
    }

    /// Whether the track at `index` reaches the master.
    pub fn track_audible(&self, index: usize) -> Option<bool> {
        self.tracks.get(index).map(|track| track.audible)
    }

    /// The first frame and length in frames of the clip that reads `slot`.
    ///
    /// Slots are one per clip, so the first clip found using `slot` is the
    /// only one; `None` means no clip reads that slot.
    pub fn slot_span(&self, slot: usize) -> Option<(u64, u64)> {
        self.tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .find(|clip| clip.slot == slot)
            .map(|clip| (clip.start, clip.frames))
    }

    /// The length of the graph in frames: one past the last clip's last frame.
    pub fn duration_frames(&self) -> u64 {
        self.tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .map(|clip| clip.end())
            .max()
            .unwrap_or(0)
    }
}

/// Builds a [`MixGraph`] on the engine thread.
///
/// Every gain, position and fade is validated and converted here, so the
/// callback's work is arithmetic on plain numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct MixGraphBuilder {
    /// The sequence sample rate.
    sample_rate: u32,
    /// Channels per frame.
    channels: u16,
    /// A floor on the slot count, independent of the clips.
    min_slots: usize,
    /// The master fader, in decibels.
    master_gain_db: f64,
    /// Whether the master is muted.
    master_muted: bool,
    /// The tracks described so far.
    tracks: Vec<TrackSpec>,
}

impl MixGraphBuilder {
    /// A graph with no tracks and an unmuted master at unity.
    #[must_use]
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            sample_rate,
            channels,
            min_slots: 0,
            master_gain_db: 0.0,
            master_muted: false,
            tracks: Vec::new(),
        }
    }

    /// Sets the master fader, in decibels.
    #[must_use]
    pub fn master_gain_db(mut self, gain_db: f64) -> Self {
        self.master_gain_db = gain_db;
        self
    }

    /// Mutes or unmutes the master bus.
    #[must_use]
    pub fn master_muted(mut self, muted: bool) -> Self {
        self.master_muted = muted;
        self
    }

    /// Reserves at least `count` ring buffer slots, even when fewer clips use
    /// them.
    #[must_use]
    pub fn slots(mut self, count: usize) -> Self {
        self.min_slots = count;
        self
    }

    /// Adds a track below the ones already described.
    #[must_use]
    pub fn track(mut self, track: TrackSpec) -> Self {
        self.tracks.push(track);
        self
    }

    /// Validates and compiles the description.
    ///
    /// # Errors
    ///
    /// Returns [`sub_core::codes::INVALID_ARGUMENT`] when the sample rate is
    /// zero, [`codes::UNSUPPORTED_LAYOUT`] when the channel count is zero or
    /// above [`MAX_CHANNELS`], [`codes::INVALID_GAIN`] for a gain outside the
    /// fader range and [`codes::GRAPH_INVALID`] for a clip whose times are
    /// negative or unrepresentable, or whose fades together outlast it.
    pub fn build(self) -> SubResult<Arc<MixGraph>> {
        let rate = sample_rate_of(self.sample_rate)?;
        if self.channels == 0 || self.channels > MAX_CHANNELS {
            return Err(SubError::new(
                codes::UNSUPPORTED_LAYOUT,
                "channel count is outside what a mixer bus can hold",
            )
            .with_detail("channels", self.channels)
            .with_detail("max_channels", MAX_CHANNELS));
        }
        let any_solo = self.tracks.iter().any(|track| track.solo);
        let mut slot_count = self.min_slots;
        let mut tracks = Vec::with_capacity(self.tracks.len());
        for spec in &self.tracks {
            let mut clips = Vec::with_capacity(spec.clips.len());
            for clip in &spec.clips {
                let node = compile_clip(clip, rate)?;
                slot_count = slot_count.max(node.slot + 1);
                clips.push(node);
            }
            tracks.push(TrackNode {
                gain: linear_gain(spec.gain_db)?,
                audible: !spec.muted && (!any_solo || spec.solo),
                clips,
            });
        }
        Ok(Arc::new(MixGraph {
            sample_rate: self.sample_rate,
            channels: self.channels,
            slot_count,
            master_gain: linear_gain(self.master_gain_db)?,
            master_muted: self.master_muted,
            any_solo,
            tracks,
        }))
    }
}

/// Validates one clip and converts its times to frames at `rate`.
fn compile_clip(clip: &ClipSpec, rate: Rational) -> SubResult<ClipNode> {
    let start = frames_at(clip.start, rate, "clip start")?;
    let frames = frames_at(clip.duration, rate, "clip duration")?;
    let fade_in = frames_at(clip.fade_in, rate, "clip fade_in")?;
    let fade_out = frames_at(clip.fade_out, rate, "clip fade_out")?;
    if fade_in.saturating_add(fade_out) > frames {
        return Err(SubError::new(
            codes::GRAPH_INVALID,
            "clip fades together may not exceed the clip length",
        )
        .with_detail("fade_in_frames", fade_in)
        .with_detail("fade_out_frames", fade_out)
        .with_detail("frames", frames));
    }
    if start.checked_add(frames).is_none() {
        return Err(SubError::new(
            codes::GRAPH_INVALID,
            "clip ends beyond the frames the mixer can count",
        )
        .with_detail("start_frames", start)
        .with_detail("frames", frames));
    }
    Ok(ClipNode {
        slot: clip.slot,
        start,
        frames,
        fade_in,
        fade_out,
        gain: linear_gain(clip.gain_db)?,
    })
}

/// One change handed from the engine thread to the callback, or handed back
/// once the callback has displaced it.
///
/// Nothing here is ever dropped on the audio thread: whatever a change
/// displaces travels back over the retire ring and is freed by
/// [`MixerControl::collect_retired`].
#[derive(Debug)]
enum MixerUpdate {
    /// A whole new graph, swapped in at the top of a callback.
    Graph(Arc<MixGraph>),
    /// The reading end of a clip's ring, or the removal of one.
    Slot {
        /// Which slot the reader belongs to.
        index: usize,
        /// The reader, or `None` to empty the slot.
        source: Option<PcmReader>,
    },
    /// A new transport position, in frames from sequence zero.
    Position(u64),
}

/// How much a [`Mixer`] preallocates.
///
/// Every buffer the callback touches is sized once, here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MixerConfig {
    /// The largest callback block, in frames. A longer request is rendered in
    /// several passes rather than allocating.
    pub max_block_frames: usize,
    /// How many clip ring slots the mixer holds. A published graph may not
    /// need more than this.
    pub slot_capacity: usize,
    /// How many pending changes the engine may queue before the callback has
    /// picked them up.
    pub queue_capacity: usize,
}

impl Default for MixerConfig {
    /// Room for a 2048-frame block, 64 clips and 32 queued changes.
    fn default() -> Self {
        Self {
            max_block_frames: 2_048,
            slot_capacity: 64,
            queue_capacity: 32,
        }
    }
}

/// Creates the two halves of a mixer: the engine's handle and the callback's.
///
/// # Errors
///
/// Returns [`sub_core::codes::INVALID_ARGUMENT`] when any capacity in `config`
/// is zero, and [`codes::GRAPH_INVALID`] when the graph needs more slots than
/// the configuration reserves.
pub fn mixer(graph: Arc<MixGraph>, config: MixerConfig) -> SubResult<(MixerControl, Mixer)> {
    for (what, value) in [
        ("max_block_frames", config.max_block_frames),
        ("slot_capacity", config.slot_capacity),
        ("queue_capacity", config.queue_capacity),
    ] {
        if value == 0 {
            return Err(SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "a mixer capacity must be greater than zero",
            )
            .with_detail("field", what));
        }
    }
    if graph.slot_count > config.slot_capacity {
        return Err(SubError::new(
            codes::GRAPH_INVALID,
            "the graph uses more clip slots than the mixer reserves",
        )
        .with_detail("slot_count", graph.slot_count)
        .with_detail("slot_capacity", config.slot_capacity));
    }
    let channels = graph.channels;
    let sample_rate = graph.sample_rate;
    let (updates_tx, updates_rx) = HeapRb::<MixerUpdate>::new(config.queue_capacity).split();
    let (retired_tx, retired_rx) = HeapRb::<MixerUpdate>::new(config.queue_capacity).split();
    let mut slots = Vec::new();
    slots.resize_with(config.slot_capacity, || None);
    let control = MixerControl {
        updates: updates_tx,
        retired: retired_rx,
        channels,
        sample_rate,
        slot_capacity: config.slot_capacity,
    };
    let mixer = Mixer {
        graph,
        slots,
        scratch: vec![0.0; config.max_block_frames * usize::from(channels)],
        track_buf: vec![0.0; config.max_block_frames * usize::from(channels)],
        meters: None,
        max_block_frames: config.max_block_frames,
        position: 0,
        underrun_frames: 0,
        updates: updates_rx,
        retired: retired_tx,
    };
    Ok((control, mixer))
}

/// The engine thread's half of a mixer.
///
/// Every method here is called off the audio thread and may allocate; none of
/// them blocks, and none of them can stall the callback.
pub struct MixerControl {
    /// Changes on their way to the callback.
    updates: ringbuf::HeapProd<MixerUpdate>,
    /// What the callback has displaced, waiting to be freed here.
    retired: ringbuf::HeapCons<MixerUpdate>,
    /// Channels per frame every graph and reader must agree on.
    channels: u16,
    /// The sample rate every graph must agree on.
    sample_rate: u32,
    /// How many slots the callback holds.
    slot_capacity: usize,
}

impl std::fmt::Debug for MixerControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MixerControl")
            .field("channels", &self.channels)
            .field("sample_rate", &self.sample_rate)
            .field("slot_capacity", &self.slot_capacity)
            .field("pending", &self.pending())
            .finish_non_exhaustive()
    }
}

impl MixerControl {
    /// Publishes a new graph. The callback swaps it in at the top of its next
    /// block; nothing is interpolated across the change.
    ///
    /// # Errors
    ///
    /// Returns [`codes::GRAPH_INVALID`] when the graph disagrees with the
    /// mixer about the sample rate or the channel count, or needs more slots
    /// than the mixer holds, and [`sub_core::codes::INVALID_STATE`] when the
    /// queue is full because the callback has not run yet.
    pub fn publish(&mut self, graph: Arc<MixGraph>) -> SubResult<()> {
        if graph.channels != self.channels || graph.sample_rate != self.sample_rate {
            return Err(SubError::new(
                codes::GRAPH_INVALID,
                "the graph does not match the mixer's stream format",
            )
            .with_detail("channels", graph.channels)
            .with_detail("sample_rate", graph.sample_rate));
        }
        if graph.slot_count > self.slot_capacity {
            return Err(SubError::new(
                codes::GRAPH_INVALID,
                "the graph uses more clip slots than the mixer reserves",
            )
            .with_detail("slot_count", graph.slot_count)
            .with_detail("slot_capacity", self.slot_capacity));
        }
        self.send(MixerUpdate::Graph(graph))
    }

    /// Hands the callback the reading end of a clip's ring for `index`.
    ///
    /// # Errors
    ///
    /// Returns [`sub_core::codes::INVALID_ARGUMENT`] when the slot is out of
    /// range, [`codes::UNSUPPORTED_LAYOUT`] when the reader's channel count is
    /// not the mixer's, and [`sub_core::codes::INVALID_STATE`] when the queue
    /// is full.
    pub fn install_slot(&mut self, index: usize, reader: PcmReader) -> SubResult<()> {
        self.check_slot(index)?;
        if reader.channels() != self.channels {
            return Err(SubError::new(
                codes::UNSUPPORTED_LAYOUT,
                "the clip ring and the mixer disagree about the channel count",
            )
            .with_detail("reader_channels", reader.channels())
            .with_detail("mixer_channels", self.channels));
        }
        self.send(MixerUpdate::Slot {
            index,
            source: Some(reader),
        })
    }

    /// Empties a slot. The ring itself is freed by
    /// [`MixerControl::collect_retired`], never on the audio thread.
    ///
    /// # Errors
    ///
    /// Returns [`sub_core::codes::INVALID_ARGUMENT`] when the slot is out of
    /// range and [`sub_core::codes::INVALID_STATE`] when the queue is full.
    pub fn clear_slot(&mut self, index: usize) -> SubResult<()> {
        self.check_slot(index)?;
        self.send(MixerUpdate::Slot {
            index,
            source: None,
        })
    }

    /// Moves the transport to `position`.
    ///
    /// The rings are not refilled here: the caller reinstalls the slots the
    /// new position needs.
    ///
    /// # Errors
    ///
    /// Returns [`codes::GRAPH_INVALID`] when the position is negative or not
    /// representable in frames, and [`sub_core::codes::INVALID_STATE`] when
    /// the queue is full.
    pub fn seek(&mut self, position: RationalTime) -> SubResult<()> {
        let rate = sample_rate_of(self.sample_rate)?;
        let frames = frames_at(position, rate, "transport position")?;
        self.send(MixerUpdate::Position(frames))
    }

    /// Frees everything the callback has handed back, returning how many
    /// values were freed.
    pub fn collect_retired(&mut self) -> usize {
        let mut freed = 0;
        while self.retired.try_pop().is_some() {
            freed += 1;
        }
        freed
    }

    /// Changes the callback has not picked up yet.
    pub fn pending(&self) -> usize {
        self.updates.occupied_len()
    }

    /// Values waiting to be freed here.
    pub fn retired(&self) -> usize {
        self.retired.occupied_len()
    }

    /// Channels per frame.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// The sequence sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// How many clip slots the callback holds.
    pub fn slot_capacity(&self) -> usize {
        self.slot_capacity
    }

    /// Rejects a slot index the callback does not have.
    fn check_slot(&self, index: usize) -> SubResult<()> {
        if index >= self.slot_capacity {
            return Err(SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "clip slot is outside the mixer's reserved slots",
            )
            .with_detail("slot", index)
            .with_detail("slot_capacity", self.slot_capacity));
        }
        Ok(())
    }

    /// Queues one change for the callback.
    fn send(&mut self, update: MixerUpdate) -> SubResult<()> {
        self.updates.try_push(update).map_err(|_| {
            SubError::new(
                sub_core::codes::INVALID_STATE,
                "the mixer update queue is full; the audio callback has not run",
            )
            .with_detail("queue_capacity", self.updates.capacity().get())
        })
    }
}

/// The scratch space one render pass borrows from the [`Mixer`], kept apart
/// from the graph it reads so the borrow checker can see the two do not
/// overlap.
struct RenderScratch<'a> {
    /// The reading end of each clip's ring, indexed by slot.
    slots: &'a mut [Option<PcmReader>],
    /// Room for one clip's frames within the block.
    clip: &'a mut [f32],
    /// Room for one track's frames within the block.
    lane: &'a mut [f32],
    /// Where levels are published, when a bank was attached.
    meters: Option<&'a MeterBank>,
}

/// The audio callback's half of a mixer.
///
/// [`Mixer::process`] is the only method the real-time thread calls, and it
/// neither locks, allocates nor frees.
pub struct Mixer {
    /// The graph in force, swapped whole when the engine publishes a new one.
    graph: Arc<MixGraph>,
    /// The reading end of each clip's ring, indexed by slot.
    slots: Vec<Option<PcmReader>>,
    /// Preallocated room for one clip's frames within a block.
    scratch: Vec<f32>,
    /// Preallocated room for one track's frames within a block. A track is
    /// summed here first so that its own level can be measured before it
    /// reaches the master.
    track_buf: Vec<f32>,
    /// Where per-track and master levels are published, when the engine
    /// attached a bank.
    meters: Option<Arc<MeterBank>>,
    /// The longest block `scratch` can hold.
    max_block_frames: usize,
    /// The transport position, in frames from sequence zero.
    position: u64,
    /// Frames a clip owed the mixer but its ring had not produced.
    underrun_frames: u64,
    /// Changes from the engine.
    updates: ringbuf::HeapCons<MixerUpdate>,
    /// Displaced values on their way back to the engine to be freed.
    retired: ringbuf::HeapProd<MixerUpdate>,
}

impl std::fmt::Debug for Mixer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mixer")
            .field("position", &self.position)
            .field("channels", &self.graph.channels)
            .field("slot_capacity", &self.slots.len())
            .field("underrun_frames", &self.underrun_frames)
            .finish_non_exhaustive()
    }
}

impl Mixer {
    /// Renders the next block into `out`, interleaved, and returns how many
    /// frames it wrote.
    ///
    /// `out` is always fully written: a clip whose ring has run dry, or a
    /// stretch of timeline with nothing on it, comes out as silence rather
    /// than as whatever the buffer held. The transport advances by the frames
    /// rendered, and a muted or soloed-out track still consumes its clips'
    /// frames so that it stays aligned.
    ///
    /// Real-time safe: no lock, no allocation, no free, no syscall.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a frame count within one block always fits a usize"
    )]
    pub fn process(&mut self, out: &mut [f32]) -> usize {
        self.apply_updates();
        out.fill(0.0);
        let channels = usize::from(self.graph.channels);
        let total = out.len() / channels;
        let mut done = 0;
        while done < total {
            let frames = (total - done).min(self.max_block_frames);
            let block = &mut out[done * channels..(done + frames) * channels];
            let mut work = RenderScratch {
                slots: &mut self.slots,
                clip: &mut self.scratch,
                lane: &mut self.track_buf,
                meters: self.meters.as_deref(),
            };
            let underruns = Self::render(&self.graph, &mut work, block, self.position, frames);
            self.underrun_frames = self.underrun_frames.saturating_add(underruns);
            self.position += frames as u64;
            done += frames;
        }
        done
    }

    /// The transport position, in frames from sequence zero.
    pub fn position_frames(&self) -> u64 {
        self.position
    }

    /// The transport position as an exact time at the sequence sample rate.
    ///
    /// Saturates at [`i64::MAX`] frames, far past any real timeline.
    pub fn position(&self) -> RationalTime {
        RationalTime::new(
            i64::try_from(self.position).unwrap_or(i64::MAX),
            self.graph.rate(),
        )
    }

    /// Frames a clip owed the mixer but its ring had not produced, counted
    /// since the mixer was created.
    pub fn underrun_frames(&self) -> u64 {
        self.underrun_frames
    }

    /// The graph in force.
    pub fn graph(&self) -> &Arc<MixGraph> {
        &self.graph
    }

    /// The longest block rendered in one pass.
    pub fn max_block_frames(&self) -> usize {
        self.max_block_frames
    }

    /// Whether the slot at `index` currently holds a ring.
    pub fn slot_installed(&self, index: usize) -> bool {
        self.slots.get(index).is_some_and(Option::is_some)
    }

    /// Publishes levels into `meters` from every block this mixer renders.
    ///
    /// Called on the engine thread before the mixer is handed to the callback.
    /// Tracks are metered by their position in the graph, post-fader, and the
    /// master after the master fader; a graph with more tracks than the bank
    /// has cells leaves the extra ones unmetered.
    #[must_use]
    pub fn with_meters(mut self, meters: Arc<MeterBank>) -> Self {
        self.meters = Some(meters);
        self
    }

    /// The bank this mixer publishes levels into, when one was attached.
    pub fn meters(&self) -> Option<&Arc<MeterBank>> {
        self.meters.as_ref()
    }

    /// Swaps in whatever the engine has published, handing back what it
    /// displaces.
    ///
    /// A change is only taken when there is room to hand its predecessor back,
    /// so nothing is ever dropped here; the rest waits for the next block.
    fn apply_updates(&mut self) {
        while self.retired.vacant_len() > 0 {
            let Some(update) = self.updates.try_pop() else {
                break;
            };
            let displaced = match update {
                MixerUpdate::Graph(graph) => Some(MixerUpdate::Graph(std::mem::replace(
                    &mut self.graph,
                    graph,
                ))),
                MixerUpdate::Slot { index, source } => {
                    // The control half rejects an out-of-range slot; the
                    // fallback only keeps the value out of this thread's hands.
                    let displaced = match self.slots.get_mut(index) {
                        Some(slot) => std::mem::replace(slot, source),
                        None => source,
                    };
                    displaced.map(|source| MixerUpdate::Slot {
                        index,
                        source: Some(source),
                    })
                }
                MixerUpdate::Position(frames) => {
                    self.position = frames;
                    None
                }
            };
            if let Some(displaced) = displaced {
                // Room was checked above, so this push always succeeds; only a
                // failure would free the value on this thread.
                drop(self.retired.try_push(displaced));
            }
        }
    }

    /// Sums every clip that overlaps the block into `out`, applies the master
    /// bus, publishes the levels and returns the frames the rings could not
    /// supply.
    ///
    /// Each track is summed into `track_buf` first: that is where its
    /// post-fader level is measured, and adding a whole track to `out` at once
    /// costs no more than adding its clips one at a time did. The master level
    /// is measured last, after the master fader, so it is what the device
    /// receives.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "an overlap within one block always fits a usize"
    )]
    fn render(
        graph: &MixGraph,
        work: &mut RenderScratch<'_>,
        out: &mut [f32],
        position: u64,
        frames: usize,
    ) -> u64 {
        let RenderScratch {
            slots,
            clip: scratch,
            lane: track_buf,
            meters,
        } = work;
        let meters = *meters;
        let channels = usize::from(graph.channels);
        let block_end = position + frames as u64;
        let samples = frames * channels;
        let mut underruns = 0u64;
        for (index, track) in graph.tracks.iter().enumerate() {
            let lane = &mut track_buf[..samples];
            lane.fill(0.0);
            for clip in &track.clips {
                let from = clip.start.max(position);
                let to = clip.end().min(block_end);
                if from >= to {
                    continue;
                }
                let wanted = (to - from) as usize;
                let Some(reader) = slots.get_mut(clip.slot).and_then(Option::as_mut) else {
                    // No ring installed yet: the clip is silent and the frames
                    // it owed count as an underrun.
                    underruns += to - from;
                    continue;
                };
                let taken = reader.read(&mut scratch[..wanted * channels]);
                underruns += (wanted - taken) as u64;
                if !track.audible {
                    // Drained above, so a muted track stays in sync.
                    continue;
                }
                let gain = clip.gain * track.gain;
                let offset = clip.offset_of(from);
                let at = (from - position) as usize;
                for frame in 0..taken {
                    let factor = gain * clip.envelope(offset + frame as u64);
                    let lane_frame = at + frame;
                    let src = &scratch[frame * channels..(frame + 1) * channels];
                    let dst = &mut lane[lane_frame * channels..(lane_frame + 1) * channels];
                    for (sample, value) in dst.iter_mut().zip(src) {
                        *sample += value * factor;
                    }
                }
            }
            if let Some(meters) = meters {
                meters.publish_track(index, levels_of(lane));
            }
            for (sample, value) in out[..samples].iter_mut().zip(lane.iter()) {
                *sample += *value;
            }
        }
        let master = if graph.master_muted {
            0.0
        } else {
            graph.master_gain
        };
        for sample in &mut out[..samples] {
            *sample *= master;
        }
        if let Some(meters) = meters {
            meters.publish_master(levels_of(&out[..samples]));
            for index in graph.tracks.len()..meters.track_capacity() {
                meters.publish_track(index, crate::meter::MeterLevels::SILENT);
            }
        }
        underruns
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sub_time::{Rational, RationalTime};

    use super::{
        ClipSpec, MixGraph, MixGraphBuilder, Mixer, MixerConfig, MixerControl, TrackSpec,
        linear_gain, mixer,
    };
    use crate::codes;
    use crate::meter::{MeterBank, MeterLevels};
    use crate::resample::{PcmWriter, pcm_ring};

    /// The 48 kHz sequence timebase every test works in.
    fn rate() -> Rational {
        Rational::from_integer(48_000).expect("a valid rate")
    }

    /// `count` frames at 48 kHz.
    fn frames(count: i64) -> RationalTime {
        RationalTime::new(count, rate())
    }

    /// A mixer over `graph` with small preallocated buffers.
    fn build(graph: Arc<MixGraph>) -> (MixerControl, Mixer) {
        mixer(
            graph,
            MixerConfig {
                max_block_frames: 64,
                slot_capacity: 4,
                queue_capacity: 4,
            },
        )
        .expect("a mixer")
    }

    /// Installs a mono ring on `slot` holding `count` frames of `value`.
    fn install(control: &mut MixerControl, slot: usize, value: f32, count: usize) -> PcmWriter {
        let (mut writer, reader) = pcm_ring(1, 512).expect("a ring");
        let filled = writer.write(&vec![value; count]);
        assert_eq!(filled, count, "the ring holds the test signal");
        control.install_slot(slot, reader).expect("install");
        writer
    }

    /// Asserts two levels match to within a quantisation step.
    fn close(left: f32, right: f32) {
        assert!(
            (left - right).abs() < 1e-4,
            "expected {right}, rendered {left}"
        );
    }

    #[test]
    fn a_mixer_without_meters_publishes_nothing() {
        let graph = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(0), frames(8))))
            .build()
            .expect("a graph");
        let (mut control, mut mixer) = build(graph);
        let _writer = install(&mut control, 0, 1.0, 8);
        assert!(mixer.meters().is_none());
        let mut out = [0.0f32; 8];
        assert_eq!(mixer.process(&mut out), 8);
    }

    #[test]
    fn track_and_master_levels_are_published_every_block() {
        let bank = Arc::new(MeterBank::new(2));
        let graph = MixGraphBuilder::new(48_000, 1)
            .master_gain_db(-6.020_6)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(0), frames(8))))
            .track(TrackSpec::new().with_clip(ClipSpec::new(1, frames(0), frames(8))))
            .build()
            .expect("a graph");
        let (mut control, mixer) = build(graph);
        let mut mixer = mixer.with_meters(Arc::clone(&bank));
        assert!(mixer.meters().is_some());
        let _first = install(&mut control, 0, 1.0, 8);
        let _second = install(&mut control, 1, 0.5, 8);

        let mut out = [0.0f32; 8];
        assert_eq!(mixer.process(&mut out), 8);

        let first = bank.track(0).expect("a cell");
        close(first.peak, 1.0);
        close(first.rms, 1.0);
        let second = bank.track(1).expect("a cell");
        close(second.peak, 0.5);
        close(second.rms, 0.5);
        // The master is measured after its fader: (1.0 + 0.5) * 0.5.
        let master = bank.master();
        close(master.peak, 0.75);
        close(master.rms, 0.75);
    }

    #[test]
    fn a_muted_track_meters_silent_while_its_ring_still_drains() {
        let bank = Arc::new(MeterBank::new(1));
        let graph = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_muted(true).with_clip(ClipSpec::new(
                0,
                frames(0),
                frames(8),
            )))
            .build()
            .expect("a graph");
        let (mut control, mixer) = build(graph);
        let mut mixer = mixer.with_meters(Arc::clone(&bank));
        let _writer = install(&mut control, 0, 1.0, 8);

        let mut out = [0.0f32; 8];
        assert_eq!(mixer.process(&mut out), 8);
        assert_eq!(bank.track(0), Some(MeterLevels::SILENT));
        assert_eq!(bank.master(), MeterLevels::SILENT);
        assert!(!mixer.slot_installed(1));
    }

    #[test]
    fn a_track_the_bank_has_no_room_for_is_simply_unmetered() {
        let bank = Arc::new(MeterBank::new(1));
        let graph = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(0), frames(8))))
            .track(TrackSpec::new().with_clip(ClipSpec::new(1, frames(0), frames(8))))
            .build()
            .expect("a graph");
        let (mut control, mixer) = build(graph);
        let mut mixer = mixer.with_meters(Arc::clone(&bank));
        let _first = install(&mut control, 0, 1.0, 8);
        let _second = install(&mut control, 1, 1.0, 8);

        let mut out = [0.0f32; 8];
        assert_eq!(mixer.process(&mut out), 8);
        close(bank.track(0).expect("a cell").peak, 1.0);
        assert_eq!(bank.track(1), None);
        close(bank.master().peak, 2.0);
    }

    #[test]
    fn a_track_that_leaves_the_graph_stops_reading_hot() {
        let bank = Arc::new(MeterBank::new(4));
        let graph = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(0), frames(8))))
            .build()
            .expect("a graph");
        let (mut control, mixer) = build(graph);
        let mut mixer = mixer.with_meters(Arc::clone(&bank));
        let _writer = install(&mut control, 0, 1.0, 8);
        let mut out = [0.0f32; 8];
        assert_eq!(mixer.process(&mut out), 8);
        close(bank.track(0).expect("a cell").peak, 1.0);

        // A cell no track occupies reads silent rather than holding the level
        // the removed track left there.
        let empty = MixGraphBuilder::new(48_000, 1).build().expect("a graph");
        control.publish(empty).expect("publish");
        assert_eq!(mixer.process(&mut out), 8);
        assert_eq!(bank.track(0), Some(MeterLevels::SILENT));
        assert_eq!(bank.master(), MeterLevels::SILENT);
    }

    #[test]
    fn clipping_shows_in_the_published_peak() {
        let bank = Arc::new(MeterBank::new(1));
        let graph = MixGraphBuilder::new(48_000, 1)
            .master_gain_db(6.0)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(0), frames(8))))
            .build()
            .expect("a graph");
        let (mut control, mixer) = build(graph);
        let mut mixer = mixer.with_meters(Arc::clone(&bank));
        let _writer = install(&mut control, 0, 0.9, 8);

        let mut out = [0.0f32; 8];
        assert_eq!(mixer.process(&mut out), 8);
        assert!(
            !bank.track(0).expect("a cell").is_clipping(),
            "the track is below full scale before the master fader"
        );
        assert!(
            bank.master().is_clipping(),
            "the master fader pushed it past full scale"
        );
    }

    #[test]
    fn clip_track_and_master_gains_multiply() {
        // -6.0206 dB three times over is a factor of one half three times over.
        let graph = MixGraphBuilder::new(48_000, 1)
            .master_gain_db(-6.020_6)
            .track(
                TrackSpec::new()
                    .with_gain_db(-6.020_6)
                    .with_clip(ClipSpec::new(0, frames(0), frames(8)).with_gain_db(-6.020_6)),
            )
            .build()
            .expect("a graph");
        let (mut control, mut mixer) = build(graph);
        let _writer = install(&mut control, 0, 1.0, 8);

        let mut out = [0.0f32; 8];
        assert_eq!(mixer.process(&mut out), 8);
        for sample in out {
            close(sample, 0.125);
        }
        assert_eq!(mixer.underrun_frames(), 0);
    }

    #[test]
    fn fades_ramp_linearly_in_amplitude() {
        let graph =
            MixGraphBuilder::new(48_000, 1)
                .track(TrackSpec::new().with_clip(
                    ClipSpec::new(0, frames(0), frames(8)).with_fades(frames(4), frames(4)),
                ))
                .build()
                .expect("a graph");
        let (mut control, mut mixer) = build(graph);
        let _writer = install(&mut control, 0, 1.0, 8);

        let mut out = [0.0f32; 8];
        assert_eq!(mixer.process(&mut out), 8);
        let expected = [0.0, 0.25, 0.5, 0.75, 1.0, 0.75, 0.5, 0.25];
        for (rendered, want) in out.iter().zip(expected) {
            close(*rendered, want);
        }
    }

    #[test]
    fn a_clip_is_silent_outside_its_own_span() {
        let graph = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(2), frames(3))))
            .build()
            .expect("a graph");
        assert_eq!(graph.duration_frames(), 5);
        let (mut control, mut mixer) = build(graph);
        let _writer = install(&mut control, 0, 1.0, 3);

        let mut out = [0.0f32; 8];
        assert_eq!(mixer.process(&mut out), 8);
        let expected = [0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0];
        for (rendered, want) in out.iter().zip(expected) {
            close(*rendered, want);
        }
        assert_eq!(mixer.underrun_frames(), 0, "nothing was owed off the clip");
    }

    #[test]
    fn a_muted_track_is_silent_but_still_drains_its_ring() {
        let graph = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_muted(true).with_clip(ClipSpec::new(
                0,
                frames(0),
                frames(8),
            )))
            .build()
            .expect("a graph");
        assert_eq!(graph.track_audible(0), Some(false));
        let (mut control, mut mixer) = build(graph);
        let writer = install(&mut control, 0, 1.0, 8);

        let mut out = [0.0f32; 4];
        assert_eq!(mixer.process(&mut out), 4);
        for sample in out {
            close(sample, 0.0);
        }
        assert_eq!(
            writer.vacant_frames(),
            512 - 4,
            "the muted track consumed its four frames"
        );
    }

    #[test]
    fn solo_silences_every_track_that_is_not_soloed() {
        let graph = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(0), frames(8))))
            .track(TrackSpec::new().with_solo(true).with_clip(ClipSpec::new(
                1,
                frames(0),
                frames(8),
            )))
            .build()
            .expect("a graph");
        assert!(graph.any_solo());
        assert_eq!(graph.track_audible(0), Some(false));
        assert_eq!(graph.track_audible(1), Some(true));
        let (mut control, mut mixer) = build(graph);
        let _muted_by_solo = install(&mut control, 0, 1.0, 8);
        let _soloed = install(&mut control, 1, 0.5, 8);

        let mut out = [0.0f32; 8];
        assert_eq!(mixer.process(&mut out), 8);
        for sample in out {
            close(sample, 0.5);
        }
    }

    #[test]
    fn a_muted_master_silences_everything() {
        let graph = MixGraphBuilder::new(48_000, 1)
            .master_muted(true)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(0), frames(8))))
            .build()
            .expect("a graph");
        assert!(graph.master_muted());
        let (mut control, mut mixer) = build(graph);
        let _writer = install(&mut control, 0, 1.0, 8);

        let mut out = [0.0f32; 8];
        assert_eq!(mixer.process(&mut out), 8);
        for sample in out {
            close(sample, 0.0);
        }
    }

    #[test]
    fn two_stereo_tracks_sum_into_the_master() {
        let graph = MixGraphBuilder::new(48_000, 2)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(0), frames(4))))
            .track(TrackSpec::new().with_clip(ClipSpec::new(1, frames(0), frames(4))))
            .build()
            .expect("a graph");
        assert_eq!(graph.clip_count(), 2);
        assert_eq!(graph.slot_count(), 2);
        assert_eq!(graph.track_count(), 2);
        assert_eq!(graph.channels(), 2);
        assert_eq!(graph.sample_rate(), 48_000);
        let (mut control, mut mixer) = build(graph);
        let mut writers = Vec::new();
        for slot in 0..2 {
            let (mut writer, reader) = pcm_ring(2, 64).expect("a ring");
            writer.write(&[0.25; 2 * 4]);
            control.install_slot(slot, reader).expect("install");
            writers.push(writer);
        }

        let mut out = [0.0f32; 2 * 4];
        assert_eq!(mixer.process(&mut out), 4);
        for sample in out {
            close(sample, 0.5);
        }
    }

    #[test]
    fn an_empty_ring_comes_out_as_silence_and_counts_underruns() {
        let graph = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(0), frames(8))))
            .build()
            .expect("a graph");
        let (mut control, mut mixer) = build(graph);
        let _writer = install(&mut control, 0, 1.0, 3);

        let mut out = [1.0f32; 8];
        assert_eq!(mixer.process(&mut out), 8);
        for (index, sample) in out.iter().enumerate() {
            close(*sample, if index < 3 { 1.0 } else { 0.0 });
        }
        assert_eq!(mixer.underrun_frames(), 5);
    }

    #[test]
    fn a_block_longer_than_the_scratch_is_rendered_in_passes() {
        let graph = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(0), frames(200))))
            .build()
            .expect("a graph");
        let (mut control, mut mixer) = build(graph);
        assert_eq!(mixer.max_block_frames(), 64);
        let _writer = install(&mut control, 0, 1.0, 200);

        let mut out = [0.0f32; 200];
        assert_eq!(mixer.process(&mut out), 200);
        for sample in out {
            close(sample, 1.0);
        }
        assert_eq!(mixer.position_frames(), 200);
        assert_eq!(mixer.underrun_frames(), 0);
    }

    #[test]
    fn publishing_swaps_the_graph_and_hands_the_old_one_back() {
        let first = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(0), frames(16))))
            .build()
            .expect("a graph");
        let (mut control, mut mixer) = build(Arc::clone(&first));
        let _writer = install(&mut control, 0, 1.0, 16);

        let mut out = [0.0f32; 4];
        assert_eq!(mixer.process(&mut out), 4);
        close(out[0], 1.0);

        // The same shape at half gain, swapped in whole.
        let second = MixGraphBuilder::new(48_000, 1)
            .track(
                TrackSpec::new()
                    .with_gain_db(-6.020_6)
                    .with_clip(ClipSpec::new(0, frames(0), frames(16))),
            )
            .build()
            .expect("a graph");
        control.publish(Arc::clone(&second)).expect("publish");
        assert_eq!(control.pending(), 1);

        assert_eq!(mixer.process(&mut out), 4);
        close(out[0], 0.5);
        assert!(Arc::ptr_eq(mixer.graph(), &second));
        assert_eq!(control.pending(), 0);
        assert_eq!(control.retired(), 1, "the old graph came back");
        assert_eq!(control.collect_retired(), 1);
        assert_eq!(
            Arc::strong_count(&first),
            1,
            "the callback holds no reference to the displaced graph"
        );
    }

    #[test]
    fn clearing_a_slot_hands_the_ring_back_to_the_engine() {
        let graph = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(0), frames(16))))
            .build()
            .expect("a graph");
        let (mut control, mut mixer) = build(graph);
        let _writer = install(&mut control, 0, 1.0, 16);

        let mut out = [0.0f32; 2];
        mixer.process(&mut out);
        assert!(mixer.slot_installed(0));
        assert_eq!(control.retired(), 0, "installing displaced nothing");

        control.clear_slot(0).expect("clear");
        mixer.process(&mut out);
        assert!(!mixer.slot_installed(0));
        assert_eq!(control.collect_retired(), 1, "the ring came back");
        close(out[0], 0.0);
        assert_eq!(mixer.underrun_frames(), 2, "a clip with no ring is silent");
    }

    #[test]
    fn seeking_moves_the_transport() {
        let graph = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(100), frames(8))))
            .build()
            .expect("a graph");
        let (mut control, mut mixer) = build(graph);
        let _writer = install(&mut control, 0, 1.0, 8);
        control.seek(frames(100)).expect("seek");

        let mut out = [0.0f32; 4];
        assert_eq!(mixer.process(&mut out), 4);
        assert_eq!(mixer.position_frames(), 104);
        assert_eq!(mixer.position(), frames(104));
        for sample in out {
            close(sample, 1.0);
        }
    }

    #[test]
    fn timeline_times_become_frame_counts_at_the_sample_rate() {
        let film = Rational::new(24, 1).expect("24 fps");
        let graph = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_clip(ClipSpec::new(
                0,
                RationalTime::new(1, film),
                RationalTime::new(2, film),
            )))
            .build()
            .expect("a graph");
        // One frame of film is exactly 2000 audio frames at 48 kHz.
        assert_eq!(graph.duration_frames(), 6_000);

        let ntsc = Rational::new(30_000, 1_001).expect("29.97 fps");
        let rounded = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_clip(ClipSpec::new(
                0,
                RationalTime::zero(ntsc),
                RationalTime::new(1, ntsc),
            )))
            .build()
            .expect("a graph");
        // 1001 / 30000 s is 1601.6 samples, rounded to the nearest frame.
        assert_eq!(rounded.duration_frames(), 1_602);
    }

    #[test]
    fn gains_convert_from_decibels() {
        close(linear_gain(0.0).expect("unity"), 1.0);
        close(linear_gain(-6.020_6).expect("half"), 0.5);
        close(linear_gain(-144.0).expect("silence"), 0.0);
        for bad in [f64::NAN, f64::INFINITY, -144.001, 24.001] {
            let err = linear_gain(bad).expect_err("out of range");
            assert_eq!(err.code, codes::INVALID_GAIN);
        }
    }

    #[test]
    fn the_builder_rejects_shapes_the_mixer_cannot_play() {
        let bad_rate = MixGraphBuilder::new(0, 2).build().expect_err("no rate");
        assert_eq!(bad_rate.code, sub_core::codes::INVALID_ARGUMENT);

        for channels in [0, 65] {
            let err = MixGraphBuilder::new(48_000, channels)
                .build()
                .expect_err("bad layout");
            assert_eq!(err.code, codes::UNSUPPORTED_LAYOUT);
        }

        let negative = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(-1), frames(8))))
            .build()
            .expect_err("negative start");
        assert_eq!(negative.code, codes::GRAPH_INVALID);

        let long_fades =
            MixGraphBuilder::new(48_000, 1)
                .track(TrackSpec::new().with_clip(
                    ClipSpec::new(0, frames(0), frames(8)).with_fades(frames(5), frames(5)),
                ))
                .build()
                .expect_err("fades outlast the clip");
        assert_eq!(long_fades.code, codes::GRAPH_INVALID);

        let loud = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_gain_db(30.0))
            .build()
            .expect_err("too loud");
        assert_eq!(loud.code, codes::INVALID_GAIN);
    }

    #[test]
    fn the_control_half_rejects_what_the_callback_cannot_take() {
        let graph = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(0), frames(8))))
            .build()
            .expect("a graph");
        let (mut control, _mixer) = build(Arc::clone(&graph));
        assert_eq!(control.slot_capacity(), 4);
        assert_eq!(control.channels(), 1);
        assert_eq!(control.sample_rate(), 48_000);

        let (_writer, reader) = pcm_ring(1, 16).expect("a ring");
        let out_of_range = control.install_slot(9, reader).expect_err("no such slot");
        assert_eq!(out_of_range.code, sub_core::codes::INVALID_ARGUMENT);

        let (_stereo_writer, stereo) = pcm_ring(2, 16).expect("a ring");
        let wrong_layout = control
            .install_slot(0, stereo)
            .expect_err("wrong channel count");
        assert_eq!(wrong_layout.code, codes::UNSUPPORTED_LAYOUT);

        let stereo_graph = MixGraphBuilder::new(48_000, 2).build().expect("a graph");
        let wrong_format = control.publish(stereo_graph).expect_err("wrong format");
        assert_eq!(wrong_format.code, codes::GRAPH_INVALID);

        let crowded = MixGraphBuilder::new(48_000, 1)
            .slots(9)
            .build()
            .expect("a graph");
        let too_many = control.publish(crowded).expect_err("too many slots");
        assert_eq!(too_many.code, codes::GRAPH_INVALID);

        // Four queued changes fill the queue; the fifth has nowhere to go.
        for _ in 0..4 {
            control.publish(Arc::clone(&graph)).expect("publish");
        }
        let full = control.publish(graph).expect_err("queue is full");
        assert_eq!(full.code, sub_core::codes::INVALID_STATE);
    }

    #[test]
    fn a_mixer_needs_room_for_the_graph_it_starts_with() {
        let graph = MixGraphBuilder::new(48_000, 1)
            .slots(8)
            .build()
            .expect("a graph");
        let err = mixer(
            graph,
            MixerConfig {
                max_block_frames: 64,
                slot_capacity: 4,
                queue_capacity: 4,
            },
        )
        .expect_err("not enough slots");
        assert_eq!(err.code, codes::GRAPH_INVALID);

        let empty = MixGraphBuilder::new(48_000, 1).build().expect("a graph");
        let err = mixer(
            empty,
            MixerConfig {
                max_block_frames: 0,
                slot_capacity: 4,
                queue_capacity: 4,
            },
        )
        .expect_err("no room to render");
        assert_eq!(err.code, sub_core::codes::INVALID_ARGUMENT);
    }
}
