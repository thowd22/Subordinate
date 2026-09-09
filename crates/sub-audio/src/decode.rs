//! Decoding audio-only files with symphonia (decision-4, docs/PLAN.md §5.4).
//!
//! GStreamer demuxes audio that lives inside a video file; a file that carries
//! nothing but audio is read here instead, in pure Rust, with no pipeline and
//! no external runtime. WAV, FLAC, MP3, AAC (ADTS and MP4) and Ogg Vorbis all
//! decode through the same path and come out as interleaved `f32` frames.
//!
//! Positions are never floats. A decoder measures time in audio frames, so
//! every [`RationalTime`] it takes or returns is exact at the file's own sample
//! rate; a caller working in another rate rescales explicitly.
//!
//! ```no_run
//! # fn main() -> sub_core::SubResult<()> {
//! use sub_audio::FileDecoder;
//! use sub_time::RationalTime;
//!
//! let mut decoder = FileDecoder::open(std::path::Path::new("/media/tone.flac"))?;
//! let rate = decoder.info().frame_rate();
//! decoder.seek(RationalTime::new(48_000, rate))?; // one second in
//! while let Some(block) = decoder.next_block()? {
//!     let _ = (block.start, block.samples);
//! }
//! # Ok(())
//! # }
//! ```

use std::fs::File;
use std::path::Path;

use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{
    FormatOptions, FormatReader, SeekMode, SeekTo, TrackType, probe::Probe,
};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{TimeBase, Timestamp};
use symphonia::default::{get_codecs, get_probe};

use sub_core::{SubError, SubResult};
use sub_time::{Rational, RationalTime, Rounding};

use crate::codes;

/// The largest channel count this decoder will hand to the mixer.
///
/// Well beyond stereo and 5.1; a file claiming more is malformed or is a
/// format this build has no business playing.
pub const MAX_CHANNELS: u16 = 64;

/// Everything a probe learned about an audio-only file.
///
/// `duration` counts whole audio frames at [`AudioInfo::frame_rate`], so it is
/// exact: no rounding happens between the container and the timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioInfo {
    /// Short name of the container, for example `wave`, `flac` or `ogg`.
    pub container: String,
    /// Human-readable container name from symphonia.
    pub container_description: String,
    /// Short name of the codec, for example `pcm_s16le` or `mp3`.
    pub codec: String,
    /// Human-readable codec name from symphonia.
    pub codec_description: String,
    /// Sample rate in hertz.
    pub sample_rate: u32,
    /// Channel count; frames are interleaved in this many samples.
    pub channels: u16,
    /// Total duration in audio frames, when the file declares one.
    pub duration: Option<RationalTime>,
    /// Whether the file can be seeked; a pipe or socket cannot.
    pub seekable: bool,
}

impl AudioInfo {
    /// The rate every [`RationalTime`] from this file is expressed at: one unit
    /// per audio frame.
    ///
    /// # Panics
    ///
    /// Never: the sample rate is checked to be non-zero when the file is
    /// opened, and a non-zero numerator over one is always a valid rational.
    pub fn frame_rate(&self) -> Rational {
        Rational::new(self.sample_rate, 1).expect("a probed sample rate is non-zero")
    }

    /// Total number of audio frames, when the file declares a duration.
    pub fn frames(&self) -> Option<u64> {
        self.duration
            .and_then(|duration| u64::try_from(duration.value()).ok())
    }
}

/// One run of decoded frames, interleaved, borrowed from the decoder.
///
/// The samples live in the decoder's own buffer, which is reused block after
/// block, so a caller that needs to keep them copies them out.
#[derive(Debug)]
pub struct Block<'a> {
    /// Position of the first frame, exact at the file's sample rate.
    pub start: RationalTime,
    /// Channel count; `samples.len()` is `frames * channels`.
    pub channels: u16,
    /// Interleaved samples in `[-1.0, 1.0]`.
    pub samples: &'a [f32],
}

impl Block<'_> {
    /// Number of audio frames in this block.
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels)
    }
}

/// A whole file decoded into memory: interleaved `f32` frames.
#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    /// Sample rate in hertz.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u16,
    /// Interleaved samples in `[-1.0, 1.0]`.
    pub samples: Vec<f32>,
}

impl Pcm {
    /// Number of audio frames held.
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels)
    }

    /// The frames as an exact [`RationalTime`] at the file's sample rate.
    ///
    /// # Panics
    ///
    /// Never: the sample rate came from a decoder that rejects a zero rate.
    pub fn duration(&self) -> RationalTime {
        let rate = Rational::new(self.sample_rate, 1).expect("a decoded sample rate is non-zero");
        RationalTime::new(i64::try_from(self.frames()).unwrap_or(i64::MAX), rate)
    }
}

/// Reads the shape of an audio-only file without decoding any samples.
///
/// # Errors
///
/// Returns [`codes::FILE_UNREADABLE`] when the path cannot be opened,
/// [`codes::UNSUPPORTED`] when no demuxer or decoder in this build understands
/// it, [`codes::NO_AUDIO_TRACK`] when the file carries no audio,
/// [`codes::UNSUPPORTED_LAYOUT`] when the track declares no usable sample rate
/// or channel count, and [`codes::DECODE_FAILED`] when the container is
/// corrupt.
pub fn probe_audio(path: &Path) -> SubResult<AudioInfo> {
    let reader = open_reader(path)?;
    let (_, info) = track_info(reader.as_ref(), path)?;
    Ok(info)
}

/// A decoder for one audio-only file.
///
/// Holds the demuxer, the codec and a reusable sample buffer. Nothing here is
/// touched from the real-time audio callback: blocks are decoded ahead of time
/// and handed to the mixer.
pub struct FileDecoder {
    reader: Box<dyn FormatReader + 'static>,
    decoder: Box<dyn AudioDecoder>,
    info: AudioInfo,
    track_id: u32,
    time_base: TimeBase,
    /// Interleaved samples of the last decoded block.
    buffer: Vec<f32>,
    /// Position of the next frame to be returned, in frames from the start.
    next_frame: u64,
    /// Frames still to be dropped after a seek landed before the target.
    skip: u64,
    /// True once the demuxer reported end of stream.
    finished: bool,
}

impl std::fmt::Debug for FileDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileDecoder")
            .field("info", &self.info)
            .field("position", &self.position())
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl FileDecoder {
    /// Opens `path` and prepares its first audio track for decoding.
    ///
    /// # Errors
    ///
    /// The same codes as [`probe_audio`], plus [`codes::UNSUPPORTED`] when the
    /// codec has no decoder registered in this build.
    pub fn open(path: &Path) -> SubResult<Self> {
        let reader = open_reader(path)?;
        let (params, info) = track_info(reader.as_ref(), path)?;
        let track_id = reader
            .first_track_known_codec(TrackType::Audio)
            .map(|track| track.id)
            .ok_or_else(|| no_audio_track(path))?;
        let time_base = reader
            .tracks()
            .iter()
            .find(|track| track.id == track_id)
            .and_then(|track| track.time_base)
            .unwrap_or_else(|| default_time_base(info.sample_rate));

        let decoder = get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(|e| symphonia_error(&e, path, "building the audio decoder"))?;

        Ok(Self {
            reader,
            decoder,
            info,
            track_id,
            time_base,
            buffer: Vec::new(),
            next_frame: 0,
            skip: 0,
            finished: false,
        })
    }

    /// What the probe found: rates, channels and duration.
    pub fn info(&self) -> &AudioInfo {
        &self.info
    }

    /// Position of the next frame [`FileDecoder::next_block`] will return.
    pub fn position(&self) -> RationalTime {
        let frames = self.next_frame.saturating_add(self.skip);
        RationalTime::new(
            i64::try_from(frames).unwrap_or(i64::MAX),
            self.info.frame_rate(),
        )
    }

    /// Decodes the next run of frames, or `None` at end of stream.
    ///
    /// Blocks are whatever size the container packets happen to be; a caller
    /// that needs fixed-size buffers accumulates them.
    ///
    /// # Errors
    ///
    /// Returns [`codes::DECODE_FAILED`] when the stream is malformed beyond
    /// recovery or changes shape mid-file, and [`codes::UNSUPPORTED`] when a
    /// packet needs a feature this build lacks.
    pub fn next_block(&mut self) -> SubResult<Option<Block<'_>>> {
        if self.finished {
            return Ok(None);
        }
        let channels = usize::from(self.info.channels);
        loop {
            let packet = match self.reader.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    self.finished = true;
                    return Ok(None);
                }
                Err(SymphoniaError::IoError(ref e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    self.finished = true;
                    return Ok(None);
                }
                Err(e) => {
                    self.finished = true;
                    return Err(symphonia_error(
                        &e,
                        Path::new(""),
                        "reading the next packet",
                    ));
                }
            };
            if packet.track_id != self.track_id {
                continue;
            }
            // Gapless playback: the encoder's leading and trailing frames are
            // not part of the media and are dropped before anything else.
            let trim_start = usize::try_from(packet.trim_start.get()).unwrap_or(usize::MAX);
            let trim_end = usize::try_from(packet.trim_end.get()).unwrap_or(usize::MAX);

            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                // A single malformed packet is skipped; the stream continues.
                Err(SymphoniaError::DecodeError(reason)) => {
                    tracing::debug!(reason, "skipping a malformed audio packet");
                    continue;
                }
                Err(e) => {
                    self.finished = true;
                    return Err(symphonia_error(
                        &e,
                        Path::new(""),
                        "decoding an audio packet",
                    ));
                }
            };
            if decoded.num_planes() != channels {
                self.finished = true;
                return Err(SubError::new(
                    codes::DECODE_FAILED,
                    "the audio stream changed channel count part way through",
                )
                .with_detail("expected_channels", channels)
                .with_detail("found_channels", decoded.num_planes()));
            }
            let Some((start, end)) = usable_range(&decoded, trim_start, trim_end, self.skip) else {
                let produced = decoded
                    .frames()
                    .saturating_sub(trim_start.saturating_add(trim_end));
                let dropped = u64::try_from(produced).unwrap_or(u64::MAX).min(self.skip);
                self.skip -= dropped;
                self.next_frame = self.next_frame.saturating_add(dropped);
                continue;
            };
            let dropped = u64::try_from(start.saturating_sub(trim_start)).unwrap_or(0);
            self.skip -= dropped.min(self.skip);
            self.next_frame = self.next_frame.saturating_add(dropped);

            decoded
                .slice(start..end)
                .copy_to_vec_interleaved(&mut self.buffer);
            let start_frame = self.next_frame;
            let frames = u64::try_from(self.buffer.len() / channels).unwrap_or(0);
            self.next_frame = self.next_frame.saturating_add(frames);
            return Ok(Some(Block {
                start: RationalTime::new(
                    i64::try_from(start_frame).unwrap_or(i64::MAX),
                    self.info.frame_rate(),
                ),
                channels: self.info.channels,
                samples: &self.buffer,
            }));
        }
    }

    /// Seeks so that the next block starts exactly at `position`.
    ///
    /// The container seeks to the nearest packet at or before the target and
    /// the residual frames are decoded and dropped, so the seek is sample
    /// accurate rather than packet accurate. Returns the position reached,
    /// which equals `position` clamped to the start of the file.
    ///
    /// # Errors
    ///
    /// Returns [`sub_core::codes::INVALID_ARGUMENT`] when `position` is
    /// negative or cannot be expressed exactly in whole frames, and
    /// [`codes::SEEK_FAILED`] when the container refuses the seek.
    pub fn seek(&mut self, position: RationalTime) -> SubResult<RationalTime> {
        let rate = self.info.frame_rate();
        let target = position
            .checked_rescaled_to_rounding(rate, Rounding::Floor)
            .ok_or_else(|| {
                SubError::new(
                    sub_core::codes::INVALID_ARGUMENT,
                    "seek position does not fit at the file's sample rate",
                )
                .with_detail("sample_rate", self.info.sample_rate)
            })?;
        let frames = u64::try_from(target.value()).map_err(|_| {
            SubError::new(
                sub_core::codes::INVALID_ARGUMENT,
                "cannot seek to a negative position",
            )
            .with_detail("frames", target.value())
        })?;

        let ts = frames_to_timestamp(frames, self.time_base, self.info.sample_rate)?;
        let seeked = self
            .reader
            .seek(
                SeekMode::Accurate,
                SeekTo::Timestamp {
                    ts,
                    track_id: self.track_id,
                },
            )
            .map_err(|e| {
                SubError::wrap(codes::SEEK_FAILED, "seeking the audio file failed", &e)
                    .with_detail("frames", frames)
            })?;
        self.decoder.reset();

        let actual = timestamp_to_frames(seeked.actual_ts, self.time_base, self.info.sample_rate);
        self.next_frame = actual.min(frames);
        self.skip = frames.saturating_sub(self.next_frame);
        self.finished = false;
        self.buffer.clear();
        Ok(RationalTime::new(
            i64::try_from(frames).unwrap_or(i64::MAX),
            rate,
        ))
    }

    /// Decodes from the current position to the end of the file into memory.
    ///
    /// # Errors
    ///
    /// The same codes as [`FileDecoder::next_block`].
    pub fn decode_to_end(&mut self) -> SubResult<Pcm> {
        let mut samples = Vec::new();
        if let Some(frames) = self.info.frames() {
            let expected = usize::try_from(frames).unwrap_or(0) * usize::from(self.info.channels);
            samples.reserve(expected);
        }
        while let Some(block) = self.next_block()? {
            samples.extend_from_slice(block.samples);
        }
        Ok(Pcm {
            sample_rate: self.info.sample_rate,
            channels: self.info.channels,
            samples,
        })
    }
}

/// Decodes a whole audio-only file into interleaved `f32` frames.
///
/// A convenience over [`FileDecoder`] for waveform generation and tests; real
/// playback streams blocks instead of holding a file in memory.
///
/// # Errors
///
/// The same codes as [`FileDecoder::open`] and [`FileDecoder::next_block`].
pub fn decode_file(path: &Path) -> SubResult<Pcm> {
    FileDecoder::open(path)?.decode_to_end()
}

/// Opens the file and finds its container.
fn open_reader(path: &Path) -> SubResult<Box<dyn FormatReader + 'static>> {
    let file = File::open(path).map_err(|e| {
        SubError::wrap(codes::FILE_UNREADABLE, "cannot open the audio file", &e)
            .with_detail("path", path.display().to_string())
    })?;
    let stream = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());

    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    let probe: &Probe = get_probe();
    probe
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| symphonia_error(&e, path, "reading the audio container"))
}

/// Reads the first audio track's codec parameters and describes the file.
fn track_info(
    reader: &(dyn FormatReader + 'static),
    path: &Path,
) -> SubResult<(AudioCodecParameters, AudioInfo)> {
    let track = reader
        .first_track_known_codec(TrackType::Audio)
        .ok_or_else(|| no_audio_track(path))?;
    let Some(CodecParameters::Audio(params)) = track.codec_params.as_ref() else {
        return Err(no_audio_track(path));
    };

    let sample_rate = params.sample_rate.filter(|rate| *rate > 0).ok_or_else(|| {
        SubError::new(
            codes::UNSUPPORTED_LAYOUT,
            "the audio track declares no sample rate",
        )
        .with_detail("path", path.display().to_string())
    })?;
    let channels = params
        .channels
        .as_ref()
        .map(symphonia::core::audio::Channels::count)
        .and_then(|count| u16::try_from(count).ok())
        .filter(|count| *count > 0 && *count <= MAX_CHANNELS)
        .ok_or_else(|| {
            SubError::new(
                codes::UNSUPPORTED_LAYOUT,
                "the audio track declares no usable channel layout",
            )
            .with_detail("path", path.display().to_string())
        })?;

    let rate = Rational::new(sample_rate, 1).expect("a non-zero sample rate is a valid rate");
    let time_base = track
        .time_base
        .unwrap_or_else(|| default_time_base(sample_rate));
    let frames = track.num_frames.or_else(|| {
        track
            .duration
            .map(|duration| ticks_to_frames(duration.get(), time_base, sample_rate))
    });
    let duration =
        frames.map(|frames| RationalTime::new(i64::try_from(frames).unwrap_or(i64::MAX), rate));

    let codec = get_codecs()
        .get_audio_decoder(params.codec)
        .map(|registered| registered.codec.info);
    let format = reader.format_info();

    Ok((
        params.clone(),
        AudioInfo {
            container: format.short_name.to_owned(),
            container_description: format.long_name.to_owned(),
            codec: codec.map_or_else(|| "unknown".to_owned(), |info| info.short_name.to_owned()),
            codec_description: codec
                .map_or_else(|| "unknown".to_owned(), |info| info.long_name.to_owned()),
            sample_rate,
            channels,
            duration,
            seekable: true,
        },
    ))
}

/// The timebase to assume when a container declares none: one tick per frame.
///
/// # Panics
///
/// Never: callers have already rejected a zero sample rate.
fn default_time_base(sample_rate: u32) -> TimeBase {
    TimeBase::try_from_recip(sample_rate).expect("a non-zero sample rate is a valid timebase")
}

/// The half-open frame range of a decoded buffer that is actually wanted,
/// after gapless trimming and after any frames a seek still has to drop.
///
/// Returns `None` when nothing in this buffer survives.
fn usable_range(
    decoded: &GenericAudioBufferRef<'_>,
    trim_start: usize,
    trim_end: usize,
    skip: u64,
) -> Option<(usize, usize)> {
    let frames = decoded.frames();
    let start = trim_start.min(frames);
    let end = frames.saturating_sub(trim_end).max(start);
    let skip = usize::try_from(skip).unwrap_or(usize::MAX);
    let start = start.saturating_add(skip).min(end);
    if start >= end {
        None
    } else {
        Some((start, end))
    }
}

/// Converts a frame count into the track's own timebase, rounding down so a
/// seek never lands after the requested frame.
fn frames_to_timestamp(frames: u64, time_base: TimeBase, sample_rate: u32) -> SubResult<Timestamp> {
    let numer = u128::from(time_base.numer.get());
    let denom = u128::from(time_base.denom.get());
    let ticks = u128::from(frames) * denom / (u128::from(sample_rate) * numer);
    i64::try_from(ticks).map(Timestamp::new).map_err(|_| {
        SubError::new(
            sub_core::codes::INVALID_ARGUMENT,
            "seek position is beyond what the container can address",
        )
        .with_detail("frames", frames)
    })
}

/// Converts a timestamp in the track's timebase into whole frames, rounding
/// down so the caller never believes it is further ahead than it is.
fn timestamp_to_frames(ts: Timestamp, time_base: TimeBase, sample_rate: u32) -> u64 {
    let Ok(ticks) = u64::try_from(ts.get()) else {
        return 0;
    };
    ticks_to_frames(ticks, time_base, sample_rate)
}

/// Converts a span in the track's timebase into whole frames, rounding down.
fn ticks_to_frames(ticks: u64, time_base: TimeBase, sample_rate: u32) -> u64 {
    let numer = u128::from(time_base.numer.get());
    let denom = u128::from(time_base.denom.get());
    let frames = u128::from(ticks) * numer * u128::from(sample_rate) / denom;
    u64::try_from(frames).unwrap_or(u64::MAX)
}

/// The error for a file that carries no decodable audio.
fn no_audio_track(path: &Path) -> SubError {
    SubError::new(codes::NO_AUDIO_TRACK, "the file has no audio track")
        .with_detail("path", path.display().to_string())
}

/// Maps a symphonia error onto a stable audio code.
fn symphonia_error(error: &SymphoniaError, path: &Path, doing: &str) -> SubError {
    let code = match error {
        SymphoniaError::Unsupported(_) => codes::UNSUPPORTED,
        SymphoniaError::SeekError(_) => codes::SEEK_FAILED,
        SymphoniaError::IoError(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            codes::DECODE_FAILED
        }
        SymphoniaError::IoError(_) => codes::FILE_UNREADABLE,
        _ => codes::DECODE_FAILED,
    };
    let mut err = SubError::wrap(code, format!("{doing} failed"), error);
    if path.as_os_str().is_empty() {
        err
    } else {
        err = err.with_detail("path", path.display().to_string());
        err
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use symphonia::core::codecs::audio::well_known::{
        CODEC_ID_AAC, CODEC_ID_FLAC, CODEC_ID_MP3, CODEC_ID_PCM_S16LE, CODEC_ID_VORBIS,
    };

    use super::{FileDecoder, MAX_CHANNELS, Pcm, decode_file, probe_audio};
    use crate::codes;
    use sub_time::{Rational, RationalTime};

    /// A scratch directory of this test binary's own, removed by the caller.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sub-audio-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// The interleaved 16-bit samples used by the synthesised fixtures: a
    /// deterministic ramp, distinct per channel, that no decoder could produce
    /// by accident.
    fn ramp(frames: usize, channels: u16) -> Vec<i16> {
        let mut samples = Vec::with_capacity(frames * usize::from(channels));
        for frame in 0..frames {
            for channel in 0..usize::from(channels) {
                let step = i16::try_from((frame * 7 + channel * 1_000) % 30_000).unwrap_or(0);
                samples.push(step - 15_000);
            }
        }
        samples
    }

    /// What decoding `samples` must produce: 16-bit PCM scales by 1/32768.
    fn expected_f32(samples: &[i16]) -> Vec<f32> {
        samples.iter().map(|s| f32::from(*s) / 32_768.0).collect()
    }

    /// Writes a 16-bit PCM WAV file, the one container this crate can author.
    fn write_wav(path: &std::path::Path, samples: &[i16], channels: u16, sample_rate: u32) {
        let bytes_per_frame = u32::from(channels) * 2;
        let data_len = u32::try_from(samples.len() * 2).expect("test fixture is small");
        let mut out = Vec::with_capacity(44 + samples.len() * 2);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16_u32.to_le_bytes());
        out.extend_from_slice(&1_u16.to_le_bytes()); // PCM
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&(sample_rate * bytes_per_frame).to_le_bytes());
        out.extend_from_slice(&u16::try_from(bytes_per_frame).unwrap().to_le_bytes());
        out.extend_from_slice(&16_u16.to_le_bytes()); // bits per sample
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, &out).expect("write wav");
    }

    #[test]
    fn a_wav_file_decodes_to_exactly_the_samples_it_holds() {
        let dir = scratch("wav-exact");
        let path = dir.join("ramp.wav");
        let samples = ramp(4_096, 2);
        write_wav(&path, &samples, 2, 48_000);

        let pcm = decode_file(&path).expect("decode the wav");
        assert_eq!(pcm.sample_rate, 48_000);
        assert_eq!(pcm.channels, 2);
        assert_eq!(pcm.frames(), 4_096);
        assert_eq!(pcm.samples, expected_f32(&samples));
        assert_eq!(
            pcm.duration(),
            RationalTime::new(4_096, Rational::new(48_000, 1).unwrap())
        );

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn a_probe_reports_duration_channels_and_rate_without_decoding() {
        let dir = scratch("probe");
        let path = dir.join("ramp.wav");
        write_wav(&path, &ramp(24_000, 1), 1, 44_100);

        let info = probe_audio(&path).expect("probe the wav");
        assert_eq!(info.channels, 1);
        assert_eq!(info.sample_rate, 44_100);
        assert_eq!(info.frames(), Some(24_000));
        assert_eq!(
            info.duration,
            Some(RationalTime::new(24_000, Rational::new(44_100, 1).unwrap()))
        );
        assert_eq!(info.container, "wave");
        assert!(info.seekable);
        assert!(!info.codec.is_empty() && !info.codec_description.is_empty());

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn a_seek_lands_on_the_exact_frame_asked_for() {
        let dir = scratch("seek");
        let path = dir.join("ramp.wav");
        let samples = ramp(9_000, 2);
        write_wav(&path, &samples, 2, 48_000);
        let rate = Rational::new(48_000, 1).unwrap();

        let mut decoder = FileDecoder::open(&path).expect("open");
        let target = RationalTime::new(3_457, rate);
        assert_eq!(decoder.seek(target).expect("seek"), target);
        assert_eq!(decoder.position(), target);

        let block = decoder
            .next_block()
            .expect("decode after the seek")
            .expect("there is audio after the seek");
        assert_eq!(block.start, target);
        assert_eq!(block.channels, 2);
        let expected = expected_f32(&samples[3_457 * 2..]);
        assert_eq!(block.samples, &expected[..block.samples.len()]);
        let first_block_frames = block.frames();

        let rest = decoder.decode_to_end().expect("decode the tail");
        assert_eq!(rest.frames(), 9_000 - 3_457 - first_block_frames);

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn seeking_back_to_zero_replays_the_whole_file() {
        let dir = scratch("seek-zero");
        let path = dir.join("ramp.wav");
        let samples = ramp(2_048, 2);
        write_wav(&path, &samples, 2, 48_000);
        let rate = Rational::new(48_000, 1).unwrap();

        let mut decoder = FileDecoder::open(&path).expect("open");
        let first = decoder.decode_to_end().expect("decode once");
        decoder.seek(RationalTime::new(0, rate)).expect("rewind");
        let second = decoder.decode_to_end().expect("decode again");
        assert_eq!(first, second);
        assert_eq!(first.samples, expected_f32(&samples));

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn seeking_to_a_negative_position_is_an_invalid_argument() {
        let dir = scratch("seek-negative");
        let path = dir.join("ramp.wav");
        write_wav(&path, &ramp(64, 2), 2, 48_000);

        let mut decoder = FileDecoder::open(&path).expect("open");
        let err = decoder
            .seek(RationalTime::new(-1, Rational::new(48_000, 1).unwrap()))
            .expect_err("a negative seek must fail");
        assert_eq!(err.code, sub_core::codes::INVALID_ARGUMENT);

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn a_missing_file_is_reported_as_unreadable() {
        let err = probe_audio(&PathBuf::from("/definitely/not/here.wav")).expect_err("must fail");
        assert_eq!(err.code, codes::FILE_UNREADABLE);
        assert!(err.details.contains_key("path"));
    }

    #[test]
    fn a_file_that_is_not_audio_is_reported_with_an_audio_code() {
        let dir = scratch("garbage");
        let path = dir.join("noise.wav");
        std::fs::write(&path, vec![0xA5_u8; 8_192]).expect("write garbage");

        let err = probe_audio(&path).expect_err("garbage is not audio");
        assert_eq!(err.code.domain(), "audio");
        assert!(
            matches!(
                err.code.as_str(),
                "audio.unsupported" | "audio.decode_failed" | "audio.no_audio_track"
            ),
            "unexpected code {}",
            err.code
        );

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn the_build_carries_a_decoder_for_every_format_the_editor_imports() {
        let codecs = symphonia::default::get_codecs();
        for (id, name) in [
            (CODEC_ID_PCM_S16LE, "wav"),
            (CODEC_ID_FLAC, "flac"),
            (CODEC_ID_MP3, "mp3"),
            (CODEC_ID_AAC, "aac"),
            (CODEC_ID_VORBIS, "ogg vorbis"),
        ] {
            assert!(
                codecs.get_audio_decoder(id).is_some(),
                "no decoder registered for {name}"
            );
        }
    }

    #[test]
    fn a_pcm_buffer_measures_itself_in_whole_frames() {
        let pcm = Pcm {
            sample_rate: 48_000,
            channels: 2,
            samples: vec![0.0; 96],
        };
        assert_eq!(pcm.frames(), 48);
        assert_eq!(
            pcm.duration(),
            RationalTime::new(48, Rational::new(48_000, 1).unwrap())
        );
    }

    #[test]
    fn a_file_with_absurdly_many_channels_is_refused() {
        let dir = scratch("channels");
        let path = dir.join("wide.wav");
        let channels = MAX_CHANNELS + 1;
        write_wav(&path, &ramp(4, channels), channels, 48_000);

        let err = probe_audio(&path).expect_err("more channels than a mixer bus can hold");
        assert_eq!(err.code.domain(), "audio");

        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
