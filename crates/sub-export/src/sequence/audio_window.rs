//! Bounded, cancellable source reads for the live audio mixer.
use std::collections::HashMap;
use std::path::Path;

use sub_audio::decode::{FileDecoder, Pcm};
use sub_core::{SubError, SubResult, codes};
use sub_media::{Decoder, DecoderOptions, StreamSelection};
use sub_model::{MediaItem, MediaUse};
use sub_time::{Rational, RationalTime, Rounding};

// Bound both source and destination allocations, independently of file length.
const MAX_SAMPLES: usize = 16 * 1024 * 1024;

/// Decodes just a source-time window, folded/resampled to the device format.
///
/// Missing samples at EOF are silence. Calls belong on a worker: opening,
/// seeking and resampling never run on the UI or device callback thread.
///
/// # Errors
/// Rejects negative/oversized windows, invalid formats, cancellation and source
/// decode errors. A window is at most 30 seconds and 64 MiB per PCM buffer.
#[allow(clippy::too_many_arguments)]
pub fn decode_media_pcm_window(
    item: &MediaItem,
    project_dir: &Path,
    stream: u16,
    sample_rate: u32,
    channels: u16,
    start: RationalTime,
    duration: RationalTime,
    cancel: &dyn Fn() -> bool,
) -> SubResult<Pcm> {
    check_cancel(cancel)?;
    if start.is_negative() || duration.is_negative() || duration > RationalTime::from_seconds(30) {
        return Err(invalid_window());
    }
    let count = sample_count(duration, sample_rate, channels)?;
    if count == 0 {
        return Ok(Pcm {
            sample_rate,
            channels,
            samples: Vec::new(),
        });
    }
    let path = item.absolute_source(project_dir, MediaUse::Export);
    let has_video = super::source_has_video(item, &path, &mut HashMap::new());
    check_cancel(cancel)?;
    let source = if has_video {
        let mut decoder = Decoder::open_with(
            &path,
            DecoderOptions {
                streams: StreamSelection::AudioOnly,
                audio_stream: stream,
                frame_timeout: std::time::Duration::from_secs(3),
                ..DecoderOptions::default()
            },
        )?;
        let format = decoder.audio_format().cloned().ok_or_else(|| {
            SubError::new(
                sub_media::codes::NO_AUDIO_STREAM,
                "no audio stream to preview",
            )
        })?;
        let mut window = Window::new(start, duration, format.sample_rate, format.channels)?;
        check_cancel(cancel)?;
        decoder.seek_audio(start)?;
        loop {
            check_cancel(cancel)?;
            let Some(block) = decoder.next_audio_block()? else {
                break;
            };
            if window.copy(block.start, block.samples)? {
                break;
            }
        }
        window.pcm
    } else {
        let mut decoder = FileDecoder::open_stream(&path, stream)?;
        let info = decoder.info();
        let mut window = Window::new(start, duration, info.sample_rate, info.channels)?;
        check_cancel(cancel)?;
        // Symphonia rejects seeks at/past a declared EOF. The requested
        // window already contains silence, so do not seek or decode there.
        let past_end = info.duration.is_some_and(|end| start >= end);
        if past_end {
            check_cancel(cancel)?;
            return Ok(Pcm {
                sample_rate,
                channels,
                samples: vec![0.0; count],
            });
        }
        decoder.seek(start)?;
        loop {
            check_cancel(cancel)?;
            let Some(block) = decoder.next_block()? else {
                break;
            };
            if window.copy(block.start, block.samples)? {
                break;
            }
        }
        window.pcm
    };
    check_cancel(cancel)?;
    // Bound the intermediate channel-fold buffer too.
    if source
        .frames()
        .checked_mul(usize::from(channels))
        .is_none_or(|n| n > MAX_SAMPLES)
    {
        return Err(invalid_window());
    }
    let mut output = super::resample(super::to_channels(source, channels), sample_rate)?;
    check_cancel(cancel)?;
    output.samples.resize(count, 0.0);
    Ok(output)
}

fn check_cancel(cancel: &dyn Fn() -> bool) -> SubResult<()> {
    if cancel() {
        Err(SubError::new(
            codes::CANCELLED,
            "audio preview decode cancelled",
        ))
    } else {
        Ok(())
    }
}

fn invalid_window() -> SubError {
    SubError::new(
        codes::INVALID_ARGUMENT,
        "audio preview window or format is out of bounds",
    )
}

fn sample_count(duration: RationalTime, rate: u32, channels: u16) -> SubResult<usize> {
    let rate = Rational::new(rate, 1).ok_or_else(invalid_window)?;
    if channels == 0 {
        return Err(invalid_window());
    }
    duration
        .checked_rescaled_to_rounding(rate, Rounding::Ceil)
        .and_then(|time| usize::try_from(time.value()).ok())
        .and_then(|frames| frames.checked_mul(usize::from(channels)))
        .filter(|count| *count <= MAX_SAMPLES)
        .ok_or_else(invalid_window)
}

struct Window {
    start: i64,
    end: i64,
    pcm: Pcm,
}

impl Window {
    fn new(
        start: RationalTime,
        duration: RationalTime,
        rate: u32,
        channels: u16,
    ) -> SubResult<Self> {
        let count = sample_count(duration, rate, channels)?;
        let frame_rate = Rational::new(rate, 1).ok_or_else(invalid_window)?;
        let start = start
            .checked_rescaled_to_rounding(frame_rate, Rounding::Floor)
            .ok_or_else(invalid_window)?
            .value();
        let frames = i64::try_from(count / usize::from(channels)).map_err(|_| invalid_window())?;
        Ok(Self {
            start,
            end: start.checked_add(frames).ok_or_else(invalid_window)?,
            pcm: Pcm {
                sample_rate: rate,
                channels,
                samples: vec![0.0; count],
            },
        })
    }

    /// Copies only the intersecting samples, retaining silence for timestamp gaps.
    /// True once this block reaches the end of the requested window.
    fn copy(&mut self, start: RationalTime, samples: &[f32]) -> SubResult<bool> {
        let channels = usize::from(self.pcm.channels);
        let block_start = start
            .checked_rescaled_to_rounding(
                Rational::new(self.pcm.sample_rate, 1).ok_or_else(invalid_window)?,
                Rounding::Floor,
            )
            .ok_or_else(invalid_window)?
            .value();
        let frames = i64::try_from(samples.len() / channels).map_err(|_| invalid_window())?;
        let block_end = block_start.checked_add(frames).ok_or_else(invalid_window)?;
        let from = self.start.max(block_start);
        let to = self.end.min(block_end);
        if to > from {
            let source =
                usize::try_from(from - block_start).map_err(|_| invalid_window())? * channels;
            let destination =
                usize::try_from(from - self.start).map_err(|_| invalid_window())? * channels;
            let count = usize::try_from(to - from).map_err(|_| invalid_window())? * channels;
            self.pcm.samples[destination..destination + count]
                .copy_from_slice(&samples[source..source + count]);
        }
        Ok(block_end >= self.end)
    }
}
