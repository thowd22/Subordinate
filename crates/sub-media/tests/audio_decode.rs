//! Decodes the audio of real video files through [`sub_media::Decoder`].
//!
//! No committed fixture carries audio — the generated set is video-only plus
//! two audio-only files — so these tests synthesise their own muxed A/V files
//! the same way `scripts/gen-fixtures.sh` does: a `gst-launch`-shaped pipeline
//! encoding colour bars beside a 440 Hz sine into Matroska, with the audio in
//! FLAC so the samples survive the round trip losslessly and a sample count can
//! be checked exactly. Each file is written once per machine into the temporary
//! directory and reused.
//!
//! Skips itself when this installation has no H.264 or FLAC encoder, so
//! `cargo test` still works on a bare checkout.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use sub_media::{AudioChannels, Decoder, DecoderOptions, StreamSelection};
use sub_time::{Rational, RationalTime};

/// Sample rate every synthesised file carries.
const RATE: u32 = 48_000;

/// Frames per audio buffer the source emits: 4800 at 48 kHz is 100 ms.
const FRAMES_PER_BUFFER: u64 = 4_800;

/// Audio buffers per file: 50 of them is exactly five seconds.
const AUDIO_BUFFERS: u64 = 50;

/// Total audio frames every synthesised file holds, exactly.
const TOTAL_FRAMES: u64 = FRAMES_PER_BUFFER * AUDIO_BUFFERS;

/// Peak of the sine `audiotestsrc` generates, on every channel.
const SOURCE_PEAK: f32 = 0.8;

/// The elements a synthesised file needs.
const REQUIRED_ELEMENTS: &[&str] = &[
    "videotestsrc",
    "audiotestsrc",
    "audioconvert",
    "x264enc",
    "h264parse",
    "flacenc",
    "matroskamux",
];

/// A synthesised file's audio shape.
#[derive(Debug, Clone, Copy)]
struct Layout {
    /// Short name, used for the file name.
    name: &'static str,
    /// Channel count the file carries.
    channels: u16,
    /// The channel mask GStreamer needs for more than two channels.
    mask: Option<u64>,
}

/// Stereo: front left and front right.
const STEREO: Layout = Layout {
    name: "stereo",
    channels: 2,
    mask: None,
};

/// Mono: one channel.
const MONO: Layout = Layout {
    name: "mono",
    channels: 1,
    mask: None,
};

/// 5.1: front pair, centre, LFE and a rear pair.
const SURROUND: Layout = Layout {
    name: "5_1",
    channels: 6,
    mask: Some(0x3f),
};

/// Held while one file is being synthesised: several tests want the same file
/// and they run in parallel.
static SYNTHESIS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Whether this installation can build the synthesis pipeline.
fn encoders_available() -> bool {
    gst::init().expect("GStreamer must initialise");
    let missing: Vec<&str> = REQUIRED_ELEMENTS
        .iter()
        .copied()
        .filter(|name| gst::ElementFactory::find(name).is_none())
        .collect();
    if missing.is_empty() {
        return true;
    }
    eprintln!("skipping: this installation lacks {}", missing.join(", "));
    false
}

/// Synthesises — or reuses — a five-second video file whose audio has the given
/// layout, and returns its path. `None` when the encoders are missing.
fn av_file(layout: Layout) -> Option<PathBuf> {
    if !encoders_available() {
        return None;
    }
    // Tests run in parallel and several want the same file; only one of them
    // may be synthesising it at a time.
    let _guard = SYNTHESIS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = std::env::temp_dir().join("sub-media-audio-fixtures");
    std::fs::create_dir_all(&dir).expect("the fixture directory must be creatable");
    let path = dir.join(format!("bars_{}_h264_flac.mkv", layout.name));
    if path.metadata().is_ok_and(|meta| meta.len() > 0) {
        return Some(path);
    }

    // Written under a temporary name and renamed, so a run that dies halfway
    // cannot leave a truncated file for the next one to decode.
    let partial = dir.join(format!("{}.{}.part", layout.name, std::process::id()));
    let channel_caps = layout.mask.map_or_else(
        || format!("channels={}", layout.channels),
        |mask| {
            format!(
                "channels={},channel-mask=(bitmask)0x{mask:x}",
                layout.channels
            )
        },
    );
    let description = format!(
        "matroskamux name=m ! filesink location={location} \
         videotestsrc pattern=smpte num-buffers=125 \
         ! video/x-raw,format=I420,width=320,height=240,framerate=25/1 \
         ! x264enc speed-preset=ultrafast ! h264parse ! m. \
         audiotestsrc wave=sine freq=440 samplesperbuffer={FRAMES_PER_BUFFER} \
         num-buffers={AUDIO_BUFFERS} \
         ! audio/x-raw,format=S16LE,rate={RATE},{channel_caps} \
         ! audioconvert ! flacenc ! m.",
        location = partial.display(),
    );
    run_to_eos(&description);
    std::fs::rename(&partial, &path).expect("the synthesised file must be renamable");
    Some(path)
}

/// Runs one `gst-launch`-shaped pipeline until it reaches end of stream.
fn run_to_eos(description: &str) {
    let pipeline = gst::parse::launch(description)
        .unwrap_or_else(|e| panic!("the synthesis pipeline must build: {e}"))
        .downcast::<gst::Pipeline>()
        .expect("parse produces a pipeline");
    pipeline
        .set_state(gst::State::Playing)
        .expect("the synthesis pipeline must start");
    let bus = pipeline.bus().expect("a pipeline has a bus");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "the synthesis pipeline timed out");
        let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(200)) else {
            continue;
        };
        match message.view() {
            gst::MessageView::Eos(_) => break,
            gst::MessageView::Error(err) => panic!("synthesis failed: {}", err.error()),
            _ => {}
        }
    }
    // EOS reaches the bus before the muxer has finished writing, so the file is
    // only complete once the pipeline is back in Null.
    pipeline
        .set_state(gst::State::Null)
        .expect("the synthesis pipeline must stop");
}

/// Opens a decoder over `path` with the given options.
fn open(path: &Path, options: DecoderOptions) -> Decoder {
    Decoder::open_with(path, options).unwrap_or_else(|e| panic!("[{}] {e}", e.code))
}

/// Options that decode audio only, in software, with a short budget.
fn audio_options(channels: AudioChannels) -> DecoderOptions {
    DecoderOptions {
        streams: StreamSelection::AudioOnly,
        audio_channels: channels,
        frame_timeout: Duration::from_secs(30),
        ..DecoderOptions::default()
    }
}

/// Every audio block of a decode, as `(start, samples)` pairs.
fn drain_audio(decoder: &mut Decoder) -> Vec<(RationalTime, Vec<f32>)> {
    let mut blocks = Vec::new();
    loop {
        match decoder.next_audio_block() {
            Ok(Some(block)) => blocks.push((block.start, block.samples.to_vec())),
            Ok(None) => return blocks,
            Err(e) => panic!("[{}] {e}", e.code),
        }
    }
}

#[test]
fn a_video_files_audio_decodes_to_interleaved_stereo_f32() {
    let Some(path) = av_file(STEREO) else {
        return;
    };
    let decoder = open(&path, audio_options(AudioChannels::StereoDownmix));
    let format = decoder.audio_format().expect("the file carries audio");
    assert_eq!(format.sample_rate, RATE, "the source rate is reported");
    assert_eq!(format.source_channels, 2);
    assert_eq!(
        format.source_layout,
        sub_media::ChannelLayout::Stereo,
        "the source layout is reported"
    );
    assert_eq!(format.channels, 2, "stereo passes through as stereo");
    assert!(!format.is_downmixed());
    assert_eq!(format.frame_rate(), Rational::HZ_48000);
}

#[test]
fn the_decoded_sample_count_matches_the_files_duration() {
    let Some(path) = av_file(STEREO) else {
        return;
    };
    let mut decoder = open(&path, audio_options(AudioChannels::StereoDownmix));
    let rate = decoder
        .audio_format()
        .expect("the file carries audio")
        .frame_rate();
    let blocks = drain_audio(&mut decoder);
    assert!(!blocks.is_empty(), "the audio decodes");

    // Positions are contiguous: every block starts where the previous ended.
    let mut expected = RationalTime::zero(rate);
    let mut frames = 0_u64;
    for (index, (start, samples)) in blocks.iter().enumerate() {
        assert_eq!(start.rate(), rate, "block {index}: rate");
        assert_eq!(*start, expected, "block {index}: position is sample-exact");
        assert_eq!(samples.len() % 2, 0, "block {index}: whole stereo frames");
        let block_frames = samples.len() as u64 / 2;
        frames += block_frames;
        expected = RationalTime::new(
            start.value() + i64::try_from(block_frames).expect("a block fits in i64"),
            rate,
        );
    }

    assert_eq!(
        frames, TOTAL_FRAMES,
        "the file holds {TOTAL_FRAMES} audio frames"
    );

    // And that count is the file's own duration, read back through the probe:
    // five seconds at 48 kHz is 240000 frames, exactly.
    let info = sub_media::probe(&path).unwrap_or_else(|e| panic!("[{}] {e}", e.code));
    let duration = info.duration.expect("the file declares a duration");
    let declared = duration.rescaled_to(rate).value();
    assert!(
        (declared - i64::try_from(frames).expect("frames fit in i64")).abs() <= 1,
        "decoded {frames} frames against a declared {declared}"
    );
}

#[test]
fn a_mono_source_folds_to_both_stereo_channels_at_unity() {
    let Some(path) = av_file(MONO) else {
        return;
    };
    let mut decoder = open(&path, audio_options(AudioChannels::StereoDownmix));
    let format = decoder
        .audio_format()
        .expect("the file carries audio")
        .clone();
    assert_eq!(format.source_channels, 1);
    assert_eq!(format.source_layout, sub_media::ChannelLayout::Mono);
    assert_eq!(format.channels, 2, "mono is delivered as stereo");
    assert!(format.is_downmixed());

    let blocks = drain_audio(&mut decoder);
    let samples: Vec<f32> = blocks.into_iter().flat_map(|(_, block)| block).collect();
    assert_eq!(
        samples.len() as u64 / 2,
        TOTAL_FRAMES,
        "the fold keeps every frame"
    );
    for (index, frame) in samples.chunks_exact(2).enumerate() {
        assert!(
            (frame[0] - frame[1]).abs() < 1e-6,
            "frame {index}: mono must land on both channels equally"
        );
    }
    let peak = samples.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
    assert!(
        (peak - SOURCE_PEAK).abs() < 0.01,
        "mono folds at unity, so the peak stays {SOURCE_PEAK}, got {peak}"
    );
}

#[test]
fn a_five_one_source_folds_with_the_documented_gains() {
    let Some(path) = av_file(SURROUND) else {
        return;
    };
    let mut decoder = open(&path, audio_options(AudioChannels::StereoDownmix));
    let format = decoder
        .audio_format()
        .expect("the file carries audio")
        .clone();
    assert_eq!(format.source_channels, 6);
    assert_eq!(format.source_layout, sub_media::ChannelLayout::Surround5_1);
    assert_eq!(format.channels, 2, "5.1 is delivered as stereo");

    let blocks = drain_audio(&mut decoder);
    let samples: Vec<f32> = blocks.into_iter().flat_map(|(_, block)| block).collect();
    assert_eq!(samples.len() as u64 / 2, TOTAL_FRAMES);
    for (index, frame) in samples.chunks_exact(2).enumerate() {
        assert!(
            (frame[0] - frame[1]).abs() < 1e-5,
            "frame {index}: the same signal on every channel folds symmetrically"
        );
    }

    // Every channel carries the same sine, so the fold is
    // front + centre + surround = 1 + 1/sqrt(2) + 1/sqrt(2) times it, and the
    // LFE — the sixth channel — contributes nothing at all.
    let expected =
        SOURCE_PEAK * (1.0 + sub_media::CENTER_DOWNMIX_GAIN + sub_media::SURROUND_DOWNMIX_GAIN);
    let peak = samples.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
    assert!(
        (peak - expected).abs() < 0.02,
        "5.1 folds to {expected}, got {peak}"
    );
}

#[test]
fn the_source_channels_can_be_taken_untouched() {
    let Some(path) = av_file(SURROUND) else {
        return;
    };
    let mut decoder = open(&path, audio_options(AudioChannels::Source));
    let format = decoder
        .audio_format()
        .expect("the file carries audio")
        .clone();
    assert_eq!(format.channels, 6, "the source channels pass through");
    assert!(!format.is_downmixed());

    let blocks = drain_audio(&mut decoder);
    let frames: u64 = blocks
        .iter()
        .map(|(_, samples)| samples.len() as u64 / 6)
        .sum();
    assert_eq!(frames, TOTAL_FRAMES);
}

#[test]
fn one_demux_feeds_both_the_video_and_the_audio_branch() {
    let Some(path) = av_file(STEREO) else {
        return;
    };
    let mut decoder = open(
        &path,
        DecoderOptions {
            streams: StreamSelection::VideoAndAudio,
            frame_timeout: Duration::from_secs(30),
            ..DecoderOptions::default()
        },
    );
    assert!(
        decoder.audio_format().is_some(),
        "the audio shape is known as soon as the file is open"
    );

    // Both branches are pulled in step, which is what a caller must do: each
    // sink holds a bounded queue.
    let mut frames = 0_u32;
    let mut audio_frames = 0_u64;
    for _ in 0..25 {
        match decoder.next_frame() {
            Ok(Some(frame)) => {
                assert!(!frame.pts().is_negative());
                frames += 1;
            }
            Ok(None) => break,
            Err(e) => panic!("[{}] {e}", e.code),
        }
        match decoder.next_audio_block() {
            Ok(Some(block)) => audio_frames += block.frames() as u64,
            Ok(None) => break,
            Err(e) => panic!("[{}] {e}", e.code),
        }
    }
    assert_eq!(frames, 25, "the video branch delivers pictures");
    assert!(
        audio_frames > 0,
        "the audio branch delivers samples off the same demux"
    );
}

#[test]
fn a_file_without_audio_cannot_be_opened_for_audio() {
    let Some(path) = sub_test_support::try_fixture("bars_1080p_h264.mp4") else {
        eprintln!("skipping: no video fixture; run scripts/gen-fixtures.sh");
        return;
    };
    let err = Decoder::open_with(
        &path,
        DecoderOptions {
            streams: StreamSelection::AudioOnly,
            frame_timeout: Duration::from_secs(30),
            ..DecoderOptions::default()
        },
    )
    .expect_err("the fixture carries no audio");
    assert_eq!(err.code.as_str(), "media.no_audio_stream");
    assert!(err.details.contains_key("path"), "the path is reported");
}

#[test]
fn a_video_only_decoder_has_no_audio_to_pull() {
    let Some(path) = av_file(STEREO) else {
        return;
    };
    let mut decoder = open(&path, DecoderOptions::default());
    assert!(decoder.audio_format().is_none());
    let err = decoder
        .next_audio_block()
        .expect_err("no audio branch was built");
    assert_eq!(err.code.as_str(), "media.no_audio_stream");
}

#[test]
fn an_audio_only_decoder_has_no_frames_to_pull() {
    let Some(path) = av_file(STEREO) else {
        return;
    };
    let mut decoder = open(&path, audio_options(AudioChannels::StereoDownmix));
    let err = decoder.next_frame().expect_err("no video branch was built");
    assert_eq!(err.code.as_str(), "media.no_video_stream");
}
