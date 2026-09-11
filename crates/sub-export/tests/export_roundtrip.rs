//! End-to-end export tests: push burned-in timecode and a click track through
//! the pipeline, then decode the file back and check the picture, the sound
//! and the duration agree (TASK-59, docs/PLAN.md §5.5).
//!
//! The picture carries its own frame number as eight black-or-white tiles, so
//! a decoded frame says which frame it is independently of its timestamp. The
//! sound carries one click at the start of every video frame's audio span. A
//! file where every decoded frame's burned-in number matches its presentation
//! time, and every click lands on the frame boundary it was written at, is in
//! sync: a drift of even one frame would move one of the two.

use std::path::Path;

use sub_audio::decode::Pcm;
use sub_audio::mixer::{ClipSpec, MixGraphBuilder, TrackSpec};
use sub_audio::offline::{OfflineSequence, PcmSource, render_audio};
use sub_export::{
    AudioCodec, Container, ExportElements, ExportSettings, PcmAudioSource, VideoCodec,
    VideoFrameSource, element_is_usable, export_with,
};
use sub_media::{AudioChannels, Decoder, DecoderOptions, HardwarePreference, StreamSelection};
use sub_time::{Rational, RationalTime};

/// Canvas of the test exports: small enough to encode in a moment, wide enough
/// for eight readable tiles.
const WIDTH: u32 = 160;
/// Canvas height.
const HEIGHT: u32 = 120;
/// How many bits of frame number the picture carries.
const TILES: u32 = 8;
/// Frames exported: one second at 25 fps.
const FRAMES: u64 = 25;
/// Sample rate of the click track.
const SAMPLE_RATE: u32 = 48_000;
/// Samples in one click.
const CLICK_SAMPLES: usize = 240;

/// Frames whose top strip spells out the frame number in binary.
struct TimecodeFrames {
    pixels: Vec<u8>,
    index: u64,
    count: u64,
}

impl TimecodeFrames {
    fn new(count: u64) -> Self {
        Self {
            pixels: vec![0; WIDTH as usize * HEIGHT as usize * 4],
            index: 0,
            count,
        }
    }

    /// Paints `index` as `TILES` tiles: white for a one, black for a zero.
    fn paint(&mut self, index: u64) {
        let tile = WIDTH / TILES;
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let bit = u32::min(x / tile, TILES - 1);
                let on = index >> bit & 1 == 1;
                let value = if on { 0xff } else { 0x00 };
                let at = (y as usize * WIDTH as usize + x as usize) * 4;
                self.pixels[at] = value;
                self.pixels[at + 1] = value;
                self.pixels[at + 2] = value;
                self.pixels[at + 3] = 0xff;
            }
        }
    }
}

impl VideoFrameSource for TimecodeFrames {
    fn next_frame(&mut self) -> sub_core::SubResult<Option<&[u8]>> {
        if self.index >= self.count {
            return Ok(None);
        }
        let index = self.index;
        self.paint(index);
        self.index += 1;
        Ok(Some(&self.pixels))
    }
}

/// Reads the burned-in frame number back out of a decoded luma plane.
fn read_timecode(luma: &[u8], stride: usize) -> u64 {
    let tile = (WIDTH / TILES) as usize;
    let row = HEIGHT as usize / 2;
    let mut index = 0;
    for bit in 0..TILES as usize {
        let x = bit * tile + tile / 2;
        if luma[row * stride + x] > 128 {
            index |= 1 << bit;
        }
    }
    index
}

/// A mono click track: `CLICK_SAMPLES` of square wave at the start of every
/// video frame's audio span, silence for the rest of it.
fn click_track(settings: &ExportSettings, frames: u64) -> Vec<f32> {
    let total = count(settings.audio_frames_through(frames));
    let mut samples = vec![0.0f32; total];
    for frame in 0..frames {
        let start = count(settings.audio_frames_through(frame));
        for offset in 0..CLICK_SAMPLES.min(total.saturating_sub(start)) {
            samples[start + offset] = if offset % 2 == 0 { 0.8 } else { -0.8 };
        }
    }
    samples
}

/// A count of frames or samples as a `usize`; every count in this test is
/// small.
fn count(frames: u64) -> usize {
    usize::try_from(frames).expect("the test counts fit in a usize")
}

/// The sample index each click actually starts at in the decoded audio.
fn click_onsets(samples: &[f32], gap: usize) -> Vec<usize> {
    let mut onsets = Vec::new();
    let mut quiet_since = 0;
    for (index, sample) in samples.iter().enumerate() {
        if sample.abs() > 0.4 {
            if index >= quiet_since {
                onsets.push(index);
            }
            quiet_since = index + gap;
        }
    }
    onsets
}

/// Whether this machine can run the elements one export needs.
fn can_export(settings: &ExportSettings) -> Option<ExportElements> {
    if sub_export::EncoderProbe::cached().is_err() {
        return None;
    }
    let elements =
        ExportElements::resolve(settings, &sub_export::EncoderPreferences::new()).ok()?;
    element_is_usable(settings.container.muxer()).then_some(elements)
}

/// A time in nanoseconds, exactly: `value * den / num` seconds.
fn nanoseconds(time: RationalTime) -> u64 {
    let value = u128::try_from(time.value().max(0)).unwrap_or(0);
    let nanos = value * u128::from(time.rate().denominator()) * 1_000_000_000
        / u128::from(time.rate().numerator());
    u64::try_from(nanos).unwrap_or(u64::MAX)
}

/// Decodes every video frame of `path` into `(frame number, presentation
/// time in nanoseconds)` pairs.
fn decode_timecodes(path: &Path) -> Vec<(u64, u64)> {
    let mut decoder = Decoder::open_with(
        path,
        DecoderOptions {
            streams: StreamSelection::Video,
            hardware: HardwarePreference::Software,
            ..DecoderOptions::default()
        },
    )
    .expect("the exported file opens");
    let mut found = Vec::new();
    while let Some(frame) = decoder.next_frame().expect("a decoded frame") {
        let luma = frame.plane_data(0).expect("a luma plane").to_vec();
        let stride = count(u64::from(frame.plane_stride(0).expect("a luma stride")));
        found.push((read_timecode(&luma, stride), nanoseconds(frame.pts())));
    }
    found
}

/// Decodes the whole audio stream of `path` into interleaved samples, with the
/// frame position the first block reported.
fn decode_audio(path: &Path) -> (i64, Vec<f32>) {
    let mut decoder = Decoder::open_with(
        path,
        DecoderOptions {
            streams: StreamSelection::AudioOnly,
            audio_channels: AudioChannels::Source,
            hardware: HardwarePreference::Software,
            ..DecoderOptions::default()
        },
    )
    .expect("the exported file opens for audio");
    let mut samples: Vec<f32> = Vec::new();
    let mut first_start = None;
    while let Some(block) = decoder.next_audio_block().expect("an audio block") {
        first_start.get_or_insert(block.start.value());
        samples.extend_from_slice(block.samples);
    }
    (first_start.unwrap_or(-1), samples)
}

/// Checks the picture: every decoded frame carries the number it was pushed
/// as, sits at the timestamp that number implies, and the stream is as long as
/// the sequence to within one frame.
fn assert_picture_is_in_time(timecodes: &[(u64, u64)], frames: u64) {
    assert_eq!(
        timecodes.len(),
        count(frames),
        "the output holds exactly the frames that were pushed"
    );
    let frame_nanos = 1_000_000_000 / frames;
    for (expected, (burned_in, pts)) in timecodes.iter().enumerate() {
        let expected = expected as u64;
        assert_eq!(
            *burned_in, expected,
            "frame {expected} carries the wrong burned-in number"
        );
        let ideal = expected * frame_nanos;
        let skew = pts.abs_diff(ideal);
        assert!(
            skew < frame_nanos,
            "frame {expected} is at {pts} ns, {skew} ns from {ideal}: more than one frame out"
        );
    }
    // The output duration equals the sequence duration within one frame.
    let last = timecodes.last().expect("a last frame").1 + frame_nanos;
    let sequence_nanos = frames * frame_nanos;
    assert!(
        last.abs_diff(sequence_nanos) < frame_nanos,
        "the output is {last} ns long, more than one frame from {sequence_nanos} ns"
    );
}

/// Checks the sound: one click per video frame, each on the frame boundary it
/// was written at, which is what proves the two branches stayed in lockstep.
fn assert_clicks_land_on_frame_boundaries(samples: &[f32], frames: u64) {
    let per_frame = count(u64::from(SAMPLE_RATE) / frames);
    let onsets = click_onsets(samples, per_frame / 2);
    assert_eq!(
        onsets.len(),
        count(frames),
        "one click per video frame came back, got {onsets:?}"
    );
    // A tenth of a frame: far tighter than the one frame the criterion asks
    // for, and tight enough that a dropped or duplicated audio block fails.
    let tolerance = per_frame / 10;
    for (frame, onset) in onsets.iter().enumerate() {
        let ideal = frame * per_frame;
        assert!(
            onset.abs_diff(ideal) <= tolerance,
            "the click for frame {frame} is at sample {onset}, {ideal} expected"
        );
    }
    assert!(
        samples.len().abs_diff(count(frames) * per_frame) <= per_frame,
        "the audio is {} samples long, more than one frame from the sequence",
        samples.len()
    );
}

#[test]
fn burned_in_timecode_and_a_click_track_come_back_in_sync() {
    let settings = ExportSettings::new(WIDTH, HEIGHT, Rational::FPS_25, Container::Mkv)
        .with_video_codec(VideoCodec::H264)
        // FLAC is lossless and has no encoder priming, so a click comes back
        // on the sample it went in on: the test measures the pipeline, not the
        // audio codec.
        .with_audio_codec(Some(AudioCodec::Flac))
        .with_audio_format(SAMPLE_RATE, 1);
    let Some(elements) = can_export(&settings) else {
        eprintln!(
            "skipping: this machine has no usable H.264 encoder, FLAC encoder or matroskamux"
        );
        return;
    };

    let dir = std::env::temp_dir().join(format!("sub-export-sync-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    let path = dir.join("sync.mkv");

    let mut frames = TimecodeFrames::new(FRAMES);
    let mut audio = PcmAudioSource::new(click_track(&settings, FRAMES));
    let mut progress = Vec::new();
    let report = export_with(
        &path,
        &settings,
        &elements,
        &mut frames,
        Some(&mut audio),
        &mut |done| progress.push(done),
    )
    .expect("the export runs");

    assert_eq!(report.video_frames, FRAMES);
    assert_eq!(
        report.audio_frames,
        settings.audio_frames_through(FRAMES),
        "the mix is pushed in lockstep: one video frame's worth of audio per frame"
    );
    assert_eq!(
        report.duration.value(),
        i64::try_from(FRAMES).expect("small")
    );
    assert_eq!(report.duration.rate(), Rational::FPS_25);
    assert_eq!(
        progress,
        (1..=FRAMES).collect::<Vec<_>>(),
        "progress is reported once per frame"
    );
    assert!(path.exists(), "the muxer wrote the file");
    assert_eq!(audio.remaining(), 0, "the whole mix was consumed");

    assert_picture_is_in_time(&decode_timecodes(&path), FRAMES);
    let (first_start, samples) = decode_audio(&path);
    assert_eq!(
        first_start, 0,
        "the audio stream starts at the first sample"
    );
    assert_clicks_land_on_frame_boundaries(&samples, FRAMES);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_offline_mixer_render_feeds_the_audio_appsrc_in_lockstep() {
    let settings = ExportSettings::new(WIDTH, HEIGHT, Rational::FPS_25, Container::Mkv)
        .with_video_codec(VideoCodec::H264)
        .with_audio_codec(Some(AudioCodec::Flac))
        .with_audio_format(SAMPLE_RATE, 1);
    let Some(elements) = can_export(&settings) else {
        eprintln!(
            "skipping: this machine has no usable H.264 encoder, FLAC encoder or matroskamux"
        );
        return;
    };

    // The mix is the real offline render of TASK-55: a click-track clip on a
    // track, mixed through the same graph playback uses, not a synthetic
    // buffer handed straight to the muxer.
    let rate = Rational::from_integer(SAMPLE_RATE).expect("a valid rate");
    let clip_frames = i64::try_from(settings.audio_frames_through(FRAMES)).expect("small");
    let graph = MixGraphBuilder::new(SAMPLE_RATE, 1)
        .track(TrackSpec::new().with_clip(ClipSpec::new(
            0,
            RationalTime::zero(rate),
            RationalTime::new(clip_frames, rate),
        )))
        .build()
        .expect("a mix graph");
    let mut sequence = OfflineSequence::new(graph);
    sequence
        .set_source(
            0,
            Box::new(PcmSource::new(Pcm {
                sample_rate: SAMPLE_RATE,
                channels: 1,
                samples: click_track(&settings, FRAMES),
            })),
        )
        .expect("the clip takes its source");
    let range = sequence.full_range();
    let mixed = render_audio(&mut sequence, range).expect("the offline render runs");
    assert_eq!(
        mixed.samples.len(),
        count(settings.audio_frames_through(FRAMES))
    );

    let dir = std::env::temp_dir().join(format!("sub-export-mix-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    let path = dir.join("mixed.mkv");

    let mut frames = TimecodeFrames::new(FRAMES);
    let mut audio = PcmAudioSource::new(mixed.samples);
    let report = export_with(
        &path,
        &settings,
        &elements,
        &mut frames,
        Some(&mut audio),
        &mut |_| (),
    )
    .expect("the export runs");
    assert_eq!(report.video_frames, FRAMES);
    assert_eq!(report.audio_frames, settings.audio_frames_through(FRAMES));
    assert_eq!(audio.remaining(), 0, "the whole render was consumed");

    assert_picture_is_in_time(&decode_timecodes(&path), FRAMES);
    let (_, samples) = decode_audio(&path);
    assert_clicks_land_on_frame_boundaries(&samples, FRAMES);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_video_only_export_writes_an_mp4_of_the_right_length() {
    let settings = ExportSettings::new(WIDTH, HEIGHT, Rational::FPS_24, Container::Mp4)
        .with_video_codec(VideoCodec::H264)
        .with_audio_codec(None);
    let Some(elements) = can_export(&settings) else {
        eprintln!("skipping: this machine has no usable H.264 encoder or mp4mux");
        return;
    };

    let dir = std::env::temp_dir().join(format!("sub-export-mp4-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    let path = dir.join("video-only.mp4");

    let mut frames = TimecodeFrames::new(12);
    let report = export_with(&path, &settings, &elements, &mut frames, None, &mut |_| ())
        .expect("the export runs");
    assert_eq!(report.video_frames, 12);
    assert_eq!(report.audio_frames, 0);
    assert_eq!(report.audio_encoder, None);
    assert_eq!(report.muxer, "mp4mux");

    let timecodes = decode_timecodes(&path);
    assert_eq!(timecodes.len(), 12);
    for (expected, (burned_in, _)) in timecodes.iter().enumerate() {
        assert_eq!(*burned_in, expected as u64);
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_short_mix_is_padded_so_the_streams_end_together() {
    let settings = ExportSettings::new(WIDTH, HEIGHT, Rational::FPS_25, Container::Mkv)
        .with_video_codec(VideoCodec::H264)
        .with_audio_codec(Some(AudioCodec::Flac))
        .with_audio_format(SAMPLE_RATE, 1);
    let Some(elements) = can_export(&settings) else {
        eprintln!(
            "skipping: this machine has no usable H.264 encoder, FLAC encoder or matroskamux"
        );
        return;
    };

    let dir = std::env::temp_dir().join(format!("sub-export-pad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    let path = dir.join("padded.mkv");

    // Half a second of mix under one second of picture.
    let mut audio = PcmAudioSource::new(click_track(&settings, FRAMES / 2));
    let mut frames = TimecodeFrames::new(FRAMES);
    let report = export_with(
        &path,
        &settings,
        &elements,
        &mut frames,
        Some(&mut audio),
        &mut |_| (),
    )
    .expect("the export runs");
    assert_eq!(
        report.audio_frames,
        settings.audio_frames_through(FRAMES),
        "the short mix is padded with silence to the end of the picture"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
