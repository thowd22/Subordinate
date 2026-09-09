//! Offline audio rendering for export (docs/PLAN.md §5.4).
//!
//! Export must produce the same mix as playback, so it runs the *same* mixer:
//! [`render_audio`] builds a real [`crate::mixer::Mixer`] over the sequence's [`MixGraph`],
//! installs a PCM ring per clip slot exactly as the engine does for playback,
//! and drives [`crate::mixer::Mixer::process`] block by block. The only difference is that
//! nothing here is real time: the rings are topped up from the clips' sources
//! before every block, so a clip can never run dry through being late and the
//! result depends on nothing but the graph and the sources.
//!
//! That makes a render deterministic and repeatable: the same sequence and the
//! same range give bit-identical samples, however long the machine took.
//!
//! Sources hand over frames that already run at the sequence sample rate and
//! channel count — the same contract the playback rings have, with the
//! conversion done upstream by [`crate::resample`].
//!
//! ```
//! # fn main() -> sub_core::SubResult<()> {
//! use sub_audio::decode::Pcm;
//! use sub_audio::mixer::{ClipSpec, MixGraphBuilder, TrackSpec};
//! use sub_audio::offline::{OfflineSequence, PcmSource, render_audio};
//! use sub_time::{Rational, RationalTime, TimeRange};
//!
//! let rate = Rational::from_integer(48_000).expect("a valid rate");
//! let graph = MixGraphBuilder::new(48_000, 1)
//!     .track(TrackSpec::new().with_clip(ClipSpec::new(
//!         0,
//!         RationalTime::zero(rate),
//!         RationalTime::new(480, rate),
//!     )))
//!     .build()?;
//!
//! let mut sequence = OfflineSequence::new(graph);
//! sequence.set_source(
//!     0,
//!     Box::new(PcmSource::new(Pcm {
//!         sample_rate: 48_000,
//!         channels: 1,
//!         samples: vec![0.5; 480],
//!     })),
//! )?;
//!
//! let range = sequence.full_range();
//! let rendered = render_audio(&mut sequence, range)?;
//! assert_eq!(rendered.frames(), 480);
//! assert_eq!(render_audio(&mut sequence, range)?, rendered);
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;

use sub_core::{SubError, SubResult};
use sub_time::TimeRange;

use crate::codes;
use crate::decode::Pcm;
use crate::mixer::{MixGraph, MixerConfig, frames_at, mixer};
use crate::resample::{PcmWriter, pcm_ring};

/// Frames rendered per [`crate::mixer::Mixer::process`] call unless the caller changes it.
///
/// The block length does not change the samples: every output frame is mixed
/// from the same clips in the same order whatever the block boundaries are.
pub const DEFAULT_BLOCK_FRAMES: usize = 1_024;

/// One clip's frames, at the sequence sample rate and channel count.
///
/// This is the offline counterpart of a playback clip ring: the renderer pulls
/// frames from it ahead of the mixer instead of a worker thread pushing them
/// in real time. Frame zero is the clip's own first frame, not sequence zero.
///
/// Implementations may allocate and block — nothing here runs on an audio
/// callback.
pub trait ClipSource: Send {
    /// The sample rate of the frames handed out, in hertz.
    fn sample_rate(&self) -> u32;

    /// Channels per frame.
    fn channels(&self) -> u16;

    /// Positions the source `frame` frames into the clip.
    ///
    /// A position past the source's last frame is not an error: the source
    /// simply has nothing left to read.
    ///
    /// # Errors
    ///
    /// Returns whatever [`SubError`] the underlying source raises when it
    /// cannot seek.
    fn seek(&mut self, frame: u64) -> SubResult<()>;

    /// Copies whole interleaved frames into `out` and returns how many.
    ///
    /// Fewer frames than asked for, zero included, means the source is
    /// exhausted; the renderer pads the clip with silence.
    ///
    /// # Errors
    ///
    /// Returns whatever [`SubError`] the underlying source raises while
    /// decoding.
    fn read(&mut self, out: &mut [f32]) -> SubResult<usize>;
}

/// A [`ClipSource`] over frames already decoded into memory.
#[derive(Debug, Clone, PartialEq)]
pub struct PcmSource {
    /// The decoded frames, at the sequence rate and channel count.
    pcm: Pcm,
    /// The next frame to hand out.
    cursor: usize,
}

impl PcmSource {
    /// A source that reads `pcm` from its first frame.
    #[must_use]
    pub fn new(pcm: Pcm) -> Self {
        Self { pcm, cursor: 0 }
    }

    /// The frames this source holds.
    pub fn frames(&self) -> usize {
        self.pcm.frames()
    }

    /// The next frame the source will hand out.
    pub fn position_frames(&self) -> usize {
        self.cursor
    }
}

impl ClipSource for PcmSource {
    fn sample_rate(&self) -> u32 {
        self.pcm.sample_rate
    }

    fn channels(&self) -> u16 {
        self.pcm.channels
    }

    fn seek(&mut self, frame: u64) -> SubResult<()> {
        self.cursor = usize::try_from(frame)
            .unwrap_or(usize::MAX)
            .min(self.pcm.frames());
        Ok(())
    }

    fn read(&mut self, out: &mut [f32]) -> SubResult<usize> {
        let channels = usize::from(self.pcm.channels);
        let wanted = out.len() / channels;
        let frames = wanted.min(self.pcm.frames() - self.cursor);
        if frames == 0 {
            return Ok(0);
        }
        let from = self.cursor * channels;
        out[..frames * channels].copy_from_slice(&self.pcm.samples[from..from + frames * channels]);
        self.cursor += frames;
        Ok(frames)
    }
}

/// The audio side of a sequence, ready to render offline: the published mix
/// graph plus the source behind each of its clip slots.
///
/// A slot with no source renders as silence, exactly as an empty ring does
/// during playback.
pub struct OfflineSequence {
    /// The graph the mixer runs, identical to the one playback publishes.
    graph: Arc<MixGraph>,
    /// One source per clip slot, indexed by slot.
    sources: Vec<Option<Box<dyn ClipSource>>>,
    /// Frames rendered per mixer block.
    block_frames: usize,
}

impl std::fmt::Debug for OfflineSequence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OfflineSequence")
            .field("sample_rate", &self.graph.sample_rate())
            .field("channels", &self.graph.channels())
            .field("slot_count", &self.graph.slot_count())
            .field("source_count", &self.source_count())
            .field("block_frames", &self.block_frames)
            .finish_non_exhaustive()
    }
}

impl OfflineSequence {
    /// A sequence over `graph` with no sources installed yet.
    #[must_use]
    pub fn new(graph: Arc<MixGraph>) -> Self {
        let mut sources = Vec::new();
        sources.resize_with(graph.slot_count(), || None);
        Self {
            graph,
            sources,
            block_frames: DEFAULT_BLOCK_FRAMES,
        }
    }

    /// Installs the source behind clip slot `slot`, replacing any earlier one.
    ///
    /// # Errors
    ///
    /// Returns [`sub_core::codes::INVALID_ARGUMENT`] when no clip in the graph
    /// reads that slot, and [`codes::UNSUPPORTED_LAYOUT`] when the source does
    /// not already run at the sequence sample rate and channel count.
    pub fn set_source(&mut self, slot: usize, source: Box<dyn ClipSource>) -> SubResult<()> {
        let Some(entry) = self.sources.get_mut(slot) else {
            return Err(SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "clip slot is outside the graph's slots",
            )
            .with_detail("slot", slot)
            .with_detail("slot_count", self.graph.slot_count()));
        };
        if source.channels() != self.graph.channels()
            || source.sample_rate() != self.graph.sample_rate()
        {
            return Err(SubError::new(
                codes::UNSUPPORTED_LAYOUT,
                "a clip source must already run at the sequence rate and channel count",
            )
            .with_detail("slot", slot)
            .with_detail("source_channels", source.channels())
            .with_detail("source_sample_rate", source.sample_rate())
            .with_detail("sequence_channels", self.graph.channels())
            .with_detail("sequence_sample_rate", self.graph.sample_rate()));
        }
        *entry = Some(source);
        Ok(())
    }

    /// The same sequence with one more source installed.
    ///
    /// # Errors
    ///
    /// As [`OfflineSequence::set_source`].
    pub fn with_source(mut self, slot: usize, source: Box<dyn ClipSource>) -> SubResult<Self> {
        self.set_source(slot, source)?;
        Ok(self)
    }

    /// The same sequence rendered in blocks of `frames`.
    ///
    /// # Errors
    ///
    /// Returns [`sub_core::codes::INVALID_ARGUMENT`] when `frames` is zero.
    pub fn with_block_frames(mut self, frames: usize) -> SubResult<Self> {
        if frames == 0 {
            return Err(SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "a render block must hold at least one frame",
            ));
        }
        self.block_frames = frames;
        Ok(self)
    }

    /// The graph the render mixes.
    pub fn graph(&self) -> &Arc<MixGraph> {
        &self.graph
    }

    /// Frames rendered per mixer block.
    pub fn block_frames(&self) -> usize {
        self.block_frames
    }

    /// How many clip slots have a source installed.
    pub fn source_count(&self) -> usize {
        self.sources.iter().filter(|slot| slot.is_some()).count()
    }

    /// The whole sequence: from zero to one past the last clip's last frame.
    ///
    /// # Panics
    ///
    /// Never: the graph's length in frames is far inside [`i64`].
    pub fn full_range(&self) -> TimeRange {
        let rate = self.graph.rate();
        let frames = i64::try_from(self.graph.duration_frames()).unwrap_or(i64::MAX);
        TimeRange::new(
            sub_time::RationalTime::zero(rate),
            sub_time::RationalTime::new(frames, rate),
        )
        .expect("a non-negative duration at the sequence rate")
    }
}

/// Renders `range` of `sequence` through the mixer and returns interleaved
/// PCM at the sequence sample rate.
///
/// The mix is the playback mix: the same [`MixGraph`], the same clip rings and
/// the same [`crate::mixer::Mixer::process`]. Because the rings are filled ahead of every
/// block, the render never underruns for want of time, and two renders of the
/// same sequence over the same range are bit-identical.
///
/// A range that starts inside a clip is handled by seeking that clip's source
/// to the matching offset, so a partial render is a sample-exact slice of the
/// whole one. Clips with no source, and sources that run out early, render as
/// silence.
///
/// # Errors
///
/// Returns [`codes::GRAPH_INVALID`] when the range is negative or not
/// representable at the sequence sample rate,
/// [`sub_core::codes::INVALID_ARGUMENT`] when the rendered samples would not
/// fit in memory, and whatever error a [`ClipSource`] raises while decoding.
pub fn render_audio(sequence: &mut OfflineSequence, range: TimeRange) -> SubResult<Pcm> {
    let graph = Arc::clone(&sequence.graph);
    let rate = graph.rate();
    let channels = usize::from(graph.channels());
    let start = frames_at(range.start(), rate, "render range start")?;
    let end = frames_at(range.end_exclusive(), rate, "render range end")?;
    let total = end.saturating_sub(start);
    let samples = usize::try_from(total)
        .ok()
        .and_then(|frames| frames.checked_mul(channels))
        .ok_or_else(|| {
            SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "the rendered range is longer than memory can hold",
            )
            .with_detail("frames", total)
            .with_detail("channels", channels)
        })?;
    let mut out = vec![0.0f32; samples];
    if total == 0 {
        return Ok(Pcm {
            sample_rate: graph.sample_rate(),
            channels: graph.channels(),
            samples: out,
        });
    }

    let block_frames = sequence.block_frames;
    let slot_capacity = graph.slot_count().max(1);
    let (mut control, mut renderer) = mixer(
        Arc::clone(&graph),
        MixerConfig {
            max_block_frames: block_frames,
            slot_capacity,
            queue_capacity: slot_capacity + 2,
        },
    )?;
    control.seek(range.start())?;

    // One ring per installed source, filled ahead of the mixer rather than by
    // a real-time worker. Each source is positioned at the frame of the clip
    // the render starts on.
    let mut writers: Vec<Option<PcmWriter>> = Vec::new();
    writers.resize_with(sequence.sources.len(), || None);
    for (slot, source) in sequence.sources.iter_mut().enumerate() {
        let Some(source) = source.as_mut() else {
            continue;
        };
        let clip_start = graph.slot_span(slot).map_or(0, |(start, _)| start);
        source.seek(start.saturating_sub(clip_start))?;
        let (writer, reader) = pcm_ring(graph.channels(), block_frames * 2)?;
        control.install_slot(slot, reader)?;
        writers[slot] = Some(writer);
    }

    let mut scratch = vec![0.0f32; block_frames * channels];
    let mut done = 0usize;
    let block_count = samples / channels;
    while done < block_count {
        let frames = block_frames.min(block_count - done);
        fill_rings(&mut sequence.sources, &mut writers, &mut scratch)?;
        let written = renderer.process(&mut out[done * channels..(done + frames) * channels]);
        debug_assert_eq!(
            written, frames,
            "the mixer always fills the block it is given"
        );
        done += frames;
    }
    if renderer.underrun_frames() > 0 {
        tracing::warn!(
            underrun_frames = renderer.underrun_frames(),
            "offline render padded clips with silence: a source ran out early"
        );
    }
    control.collect_retired();

    Ok(Pcm {
        sample_rate: graph.sample_rate(),
        channels: graph.channels(),
        samples: out,
    })
}

/// Tops every installed ring up from its source before the next block.
fn fill_rings(
    sources: &mut [Option<Box<dyn ClipSource>>],
    writers: &mut [Option<PcmWriter>],
    scratch: &mut [f32],
) -> SubResult<()> {
    for (source, writer) in sources.iter_mut().zip(writers.iter_mut()) {
        let (Some(source), Some(writer)) = (source.as_mut(), writer.as_mut()) else {
            continue;
        };
        let channels = usize::from(writer.channels());
        let chunk = scratch.len() / channels;
        while writer.vacant_frames() > 0 {
            let wanted = writer.vacant_frames().min(chunk);
            let taken = source.read(&mut scratch[..wanted * channels])?;
            if taken == 0 {
                break;
            }
            writer.write(&scratch[..taken * channels]);
            if taken < wanted {
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sub_time::{Rational, RationalTime, TimeRange};

    use super::{ClipSource, OfflineSequence, PcmSource, render_audio};
    use crate::codes;
    use crate::decode::Pcm;
    use crate::mixer::{
        ClipSpec, MixGraph, MixGraphBuilder, MixerConfig, TrackSpec, linear_gain, mixer,
    };
    use crate::resample::pcm_ring;

    /// The 48 kHz sequence timebase every test works in.
    fn rate() -> Rational {
        Rational::from_integer(48_000).expect("a valid rate")
    }

    /// `count` frames at 48 kHz.
    fn frames(count: i64) -> RationalTime {
        RationalTime::new(count, rate())
    }

    /// A range of `count` frames from `from`.
    fn range(from: i64, count: i64) -> TimeRange {
        TimeRange::new(frames(from), frames(count)).expect("a valid range")
    }

    /// A mono source whose frame `i` holds `first + i` scaled down, so every
    /// frame is distinguishable from every other.
    #[allow(
        clippy::cast_precision_loss,
        reason = "test fixtures stay far below f32's exact integer range"
    )]
    fn ramp(count: usize, first: f32) -> Box<dyn ClipSource> {
        let samples = (0..count).map(|i| first + i as f32 / 4_096.0).collect();
        Box::new(PcmSource::new(Pcm {
            sample_rate: 48_000,
            channels: 1,
            samples,
        }))
    }

    /// A mono source of `count` frames all holding `value`.
    fn flat(count: usize, value: f32) -> Box<dyn ClipSource> {
        Box::new(PcmSource::new(Pcm {
            sample_rate: 48_000,
            channels: 1,
            samples: vec![value; count],
        }))
    }

    /// A two-track graph: a faded clip over a gained one, master trimmed.
    fn mixed_graph() -> Arc<MixGraph> {
        MixGraphBuilder::new(48_000, 1)
            .master_gain_db(-2.0)
            .track(
                TrackSpec::new().with_gain_db(-3.0).with_clip(
                    ClipSpec::new(0, frames(0), frames(600))
                        .with_fades(frames(100), frames(150))
                        .with_gain_db(-1.5),
                ),
            )
            .track(
                TrackSpec::new()
                    .with_clip(ClipSpec::new(1, frames(200), frames(600)).with_gain_db(2.0)),
            )
            .build()
            .expect("a valid graph")
    }

    /// The graph above with both clips fed from ramps.
    fn mixed_sequence() -> OfflineSequence {
        OfflineSequence::new(mixed_graph())
            .with_source(0, ramp(600, 0.1))
            .expect("slot 0")
            .with_source(1, ramp(600, -0.3))
            .expect("slot 1")
    }

    #[test]
    fn two_renders_of_the_same_sequence_are_bit_identical() {
        let mut sequence = mixed_sequence();
        let whole = sequence.full_range();
        let first = render_audio(&mut sequence, whole).expect("a render");
        let second = render_audio(&mut sequence, whole).expect("a second render");
        assert_eq!(first.sample_rate, 48_000);
        assert_eq!(first.frames(), 800);
        assert!(
            first
                .samples
                .iter()
                .zip(&second.samples)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "repeat renders must be bit-identical"
        );
    }

    #[test]
    fn render_matches_the_playback_mixer_sample_for_sample() {
        let graph = mixed_graph();
        let mut sequence = OfflineSequence::new(Arc::clone(&graph))
            .with_source(0, ramp(600, 0.1))
            .expect("slot 0")
            .with_source(1, ramp(600, -0.3))
            .expect("slot 1");
        let whole = sequence.full_range();
        let rendered = render_audio(&mut sequence, whole).expect("a render");

        // The same graph driven by hand through the playback mixer, with
        // rings large enough that nothing underruns.
        let (mut control, mut playback) = mixer(
            graph,
            MixerConfig {
                max_block_frames: 256,
                slot_capacity: 2,
                queue_capacity: 8,
            },
        )
        .expect("a mixer");
        let mut writers = Vec::new();
        for (slot, first) in [(0usize, 0.1f32), (1, -0.3)] {
            let (mut writer, reader) = pcm_ring(1, 1_024).expect("a ring");
            let mut source = ramp(600, first);
            let mut buffer = [0.0f32; 600];
            let taken = source.read(&mut buffer).expect("frames");
            assert_eq!(taken, 600);
            assert_eq!(writer.write(&buffer), 600);
            control.install_slot(slot, reader).expect("a slot");
            writers.push(writer);
        }
        let mut expected = vec![0.0f32; 800];
        assert_eq!(playback.process(&mut expected), 800);
        drop(writers);

        assert!(
            rendered
                .samples
                .iter()
                .zip(&expected)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "the offline render must equal the playback mix"
        );
    }

    #[test]
    #[allow(
        clippy::cast_precision_loss,
        reason = "fade lengths are far below f32's exact integer range"
    )]
    fn a_rendered_fade_follows_the_expected_gain_curve() {
        let fade_in = 100i64;
        let fade_out = 150i64;
        let length = 600i64;
        let clip_gain_db = -4.0;
        let graph = MixGraphBuilder::new(48_000, 1)
            .track(
                TrackSpec::new().with_clip(
                    ClipSpec::new(0, frames(0), frames(length))
                        .with_fades(frames(fade_in), frames(fade_out))
                        .with_gain_db(clip_gain_db),
                ),
            )
            .build()
            .expect("a valid graph");
        let mut sequence = OfflineSequence::new(graph)
            .with_source(0, flat(usize::try_from(length).expect("a length"), 1.0))
            .expect("slot 0");

        let whole = sequence.full_range();
        let rendered = render_audio(&mut sequence, whole).expect("a render");
        let gain = linear_gain(clip_gain_db).expect("a gain");
        for (index, sample) in rendered.samples.iter().enumerate() {
            let offset = i64::try_from(index).expect("a frame index");
            let mut expected = gain;
            if offset < fade_in {
                expected *= offset as f32 / fade_in as f32;
            }
            let remaining = length - offset;
            if remaining <= fade_out {
                expected *= remaining as f32 / fade_out as f32;
            }
            assert!(
                (sample - expected).abs() < 1e-6,
                "frame {index}: rendered {sample} but the fade curve wants {expected}"
            );
        }
        assert!(
            rendered.samples[0].abs() < f32::EPSILON,
            "a fade in starts from silence"
        );
        assert!(
            (rendered.samples[usize::try_from(fade_in).expect("a length")] - gain).abs() < 1e-6,
            "a fade in reaches unity at its end"
        );
    }

    #[test]
    fn a_sub_range_is_a_slice_of_the_whole_render() {
        let mut sequence = mixed_sequence();
        let whole = sequence.full_range();
        let all = render_audio(&mut sequence, whole).expect("a render");
        let part = render_audio(&mut sequence, range(250, 300)).expect("a partial render");
        assert_eq!(part.frames(), 300);
        assert!(
            part.samples
                .iter()
                .zip(&all.samples[250..550])
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "a partial render must be a slice of the whole one"
        );
    }

    #[test]
    fn the_block_length_does_not_change_the_samples() {
        let mut small = mixed_sequence()
            .with_block_frames(17)
            .expect("a block length");
        let mut large = mixed_sequence()
            .with_block_frames(4_096)
            .expect("a block length");
        let whole = small.full_range();
        let from_small = render_audio(&mut small, whole).expect("a render");
        let from_large = render_audio(&mut large, whole).expect("a render");
        assert_eq!(from_small, from_large);
    }

    #[test]
    fn a_slot_with_no_source_renders_silence() {
        let mut sequence = OfflineSequence::new(mixed_graph())
            .with_source(1, ramp(600, -0.3))
            .expect("slot 1");
        assert_eq!(sequence.source_count(), 1);
        let rendered = render_audio(&mut sequence, range(0, 200)).expect("a render");
        assert!(
            rendered.samples.iter().all(|sample| *sample == 0.0),
            "an uninstalled slot contributes nothing"
        );
    }

    #[test]
    fn a_source_shorter_than_its_clip_is_padded_with_silence() {
        let graph = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_clip(ClipSpec::new(0, frames(0), frames(400))))
            .build()
            .expect("a valid graph");
        let mut sequence = OfflineSequence::new(graph)
            .with_source(0, flat(100, 0.25))
            .expect("slot 0");
        let whole = sequence.full_range();
        let rendered = render_audio(&mut sequence, whole).expect("a render");
        assert_eq!(rendered.frames(), 400);
        assert!(
            rendered.samples[..100]
                .iter()
                .all(|s| s.to_bits() == 0.25f32.to_bits())
        );
        assert!(rendered.samples[100..].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn an_empty_range_renders_no_frames() {
        let mut sequence = mixed_sequence();
        let rendered = render_audio(&mut sequence, range(100, 0)).expect("a render");
        assert_eq!(rendered.frames(), 0);
        assert!(rendered.samples.is_empty());
    }

    #[test]
    fn a_negative_range_is_rejected() {
        let mut sequence = mixed_sequence();
        let bad = TimeRange::new(frames(-10), frames(100)).expect("a valid range");
        let error = render_audio(&mut sequence, bad).expect_err("a negative start is rejected");
        assert_eq!(error.code, codes::GRAPH_INVALID);
    }

    #[test]
    fn a_source_that_does_not_match_the_sequence_is_rejected() {
        let mut sequence = OfflineSequence::new(mixed_graph());
        let stereo = Box::new(PcmSource::new(Pcm {
            sample_rate: 48_000,
            channels: 2,
            samples: vec![0.0; 200],
        }));
        let error = sequence
            .set_source(0, stereo)
            .expect_err("a channel mismatch is rejected");
        assert_eq!(error.code, codes::UNSUPPORTED_LAYOUT);

        let wrong_rate = Box::new(PcmSource::new(Pcm {
            sample_rate: 44_100,
            channels: 1,
            samples: vec![0.0; 200],
        }));
        let error = sequence
            .set_source(0, wrong_rate)
            .expect_err("a rate mismatch is rejected");
        assert_eq!(error.code, codes::UNSUPPORTED_LAYOUT);

        let error = sequence
            .set_source(9, flat(10, 0.0))
            .expect_err("an unknown slot is rejected");
        assert_eq!(error.code, sub_core::codes::INVALID_ARGUMENT);
    }

    #[test]
    fn a_pcm_source_seeks_within_the_clip() {
        let mut source = PcmSource::new(Pcm {
            sample_rate: 48_000,
            channels: 1,
            samples: vec![0.0, 1.0, 2.0, 3.0],
        });
        source.seek(2).expect("a seek");
        assert_eq!(source.position_frames(), 2);
        let mut out = [0.0f32; 4];
        assert_eq!(source.read(&mut out).expect("frames"), 2);
        assert_eq!(out[..2], [2.0, 3.0]);
        assert_eq!(source.read(&mut out).expect("frames"), 0);
        source.seek(99).expect("a seek past the end");
        assert_eq!(source.position_frames(), source.frames());
    }

    #[test]
    fn a_muted_track_and_a_soloed_track_render_as_they_play() {
        let graph = MixGraphBuilder::new(48_000, 1)
            .track(TrackSpec::new().with_muted(true).with_clip(ClipSpec::new(
                0,
                frames(0),
                frames(200),
            )))
            .track(TrackSpec::new().with_solo(true).with_clip(ClipSpec::new(
                1,
                frames(0),
                frames(200),
            )))
            .track(TrackSpec::new().with_clip(ClipSpec::new(2, frames(0), frames(200))))
            .build()
            .expect("a valid graph");
        let mut sequence = OfflineSequence::new(graph)
            .with_source(0, flat(200, 0.5))
            .expect("slot 0")
            .with_source(1, flat(200, 0.25))
            .expect("slot 1")
            .with_source(2, flat(200, 0.125))
            .expect("slot 2");
        let whole = sequence.full_range();
        let rendered = render_audio(&mut sequence, whole).expect("a render");
        assert!(
            rendered
                .samples
                .iter()
                .all(|sample| sample.to_bits() == 0.25f32.to_bits()),
            "only the soloed track reaches the master"
        );
    }
}
