//! Real, bounded audio reads across both source decoder paths.
use gst::prelude::*;
use gstreamer as gst;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use sub_audio::FileDecoder;
use sub_export::sequence::decode_media_pcm_window;
use sub_media::{Decoder, DecoderOptions, StreamSelection};
use sub_model::{MediaItem, MediaPath};
use sub_time::{Rational, RationalTime};

fn time(frames: i64) -> RationalTime {
    RationalTime::new(frames, Rational::HZ_48000)
}

fn fixtures() -> Option<&'static [PathBuf; 2]> {
    static FILES: OnceLock<Option<[PathBuf; 2]>> = OnceLock::new();
    FILES.get_or_init(|| {
        gst::init().unwrap();
        for name in ["audiotestsrc", "wavenc", "videotestsrc", "x264enc", "h264parse", "flacenc", "matroskamux"] {
            if gst::ElementFactory::find(name).is_none() {
                eprintln!("skipping audio windows: missing {name}"); return None;
            }
        }
        let dir = std::env::temp_dir().join(format!("sub-window-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let files = [dir.join("tone.wav"), dir.join("av.mkv")];
        for (i, path) in files.iter().enumerate() {
            let audio = "audiotestsrc wave=sine freq=437 num-buffers=20 samplesperbuffer=4800 ! audio/x-raw,format=S16LE,channels=2,rate=48000";
            let location = path.display().to_string().replace('\\', "/");
            let description = if i == 0 { format!("{audio} ! wavenc ! filesink location=\"{location}\"") } else {
                format!("matroskamux name=m ! filesink location=\"{location}\" videotestsrc num-buffers=50 ! video/x-raw,format=I420,width=64,height=64,framerate=25/1 ! x264enc speed-preset=ultrafast ! h264parse ! m. {audio} ! flacenc ! m.")
            };
            let pipeline = gst::parse::launch(&description).unwrap().downcast::<gst::Pipeline>().unwrap();
            pipeline.set_state(gst::State::Playing).unwrap();
            let result = pipeline.bus().unwrap().timed_pop_filtered(gst::ClockTime::from_seconds(30), &[gst::MessageType::Eos, gst::MessageType::Error]);
            pipeline.set_state(gst::State::Null).unwrap();
            let message = result.expect("fixture synthesis timed out");
            assert!(matches!(message.view(), gst::MessageView::Eos(_)), "{message:?}");
        }
        Some(files)
    }).as_ref()
}

fn item(path: &Path) -> MediaItem {
    MediaItem::new(MediaPath::external(path).unwrap())
}

fn full_reference(path: &Path) -> Vec<f32> {
    let mut samples = Vec::new();
    if path.extension().unwrap() == "wav" {
        let mut decoder = FileDecoder::open(path).unwrap();
        while let Some(block) = decoder.next_block().unwrap() {
            samples.extend_from_slice(block.samples);
        }
    } else {
        let mut decoder = Decoder::open_with(
            path,
            DecoderOptions {
                streams: StreamSelection::AudioOnly,
                frame_timeout: Duration::from_secs(3),
                ..DecoderOptions::default()
            },
        )
        .unwrap();
        while let Some(block) = decoder.next_audio_block().unwrap() {
            samples.extend_from_slice(block.samples);
        }
    }
    assert_eq!(samples.len(), 96_000 * 2);
    samples
}

#[test]
fn nonzero_windows_match_full_decode_samples_for_wav_and_av() {
    let Some(files) = fixtures() else { return };
    for path in files {
        let reference = full_reference(path);
        // Neither boundary is aligned to a source packet or FLAC block.
        let start = 17_123;
        let count = 8_137;
        let pcm = decode_media_pcm_window(
            &item(path),
            Path::new("."),
            0,
            48_000,
            2,
            time(start),
            time(count),
            &|| false,
        )
        .unwrap();
        assert_samples(
            &pcm.samples,
            &reference
                [usize::try_from(start).unwrap() * 2..usize::try_from(start + count).unwrap() * 2],
            path,
        );
    }
}

#[test]
fn converted_windows_have_exact_bounded_format_and_audible_samples() {
    let Some(files) = fixtures() else { return };
    for path in files {
        let pcm = decode_media_pcm_window(
            &item(path),
            Path::new("."),
            0,
            24_000,
            1,
            time(12_000),
            time(12_001),
            &|| false,
        )
        .unwrap();
        assert_eq!(
            (pcm.sample_rate, pcm.channels, pcm.samples.len()),
            (24_000, 1, 6_001)
        );
        assert!(pcm.samples.iter().all(|sample| sample.is_finite()));
        assert!(pcm.samples.iter().any(|sample| sample.abs() > 0.1));
    }
}

#[test]
fn windows_crossing_eof_keep_the_remaining_audio_and_pad_silence() {
    let Some(files) = fixtures() else { return };
    for path in files {
        let reference = full_reference(path);
        let pcm = decode_media_pcm_window(
            &item(path),
            Path::new("."),
            0,
            48_000,
            2,
            time(95_123),
            time(2_000),
            &|| false,
        )
        .unwrap();
        assert_eq!(pcm.samples.len(), 4_000);
        assert_samples(&pcm.samples[..877 * 2], &reference[95_123 * 2..], path);
        assert!(pcm.samples[877 * 2..].iter().all(|sample| *sample == 0.0));
    }
}

#[test]
fn cancelled_and_invalid_windows_fail_before_opening_the_source() {
    let item = MediaItem::new(MediaPath::new("does-not-exist.wav").unwrap());
    let call = |rate, channels, start, duration, cancel: &dyn Fn() -> bool| {
        decode_media_pcm_window(
            &item,
            Path::new("."),
            0,
            rate,
            channels,
            start,
            duration,
            cancel,
        )
    };
    let error = call(48_000, 2, time(0), time(480), &|| true).unwrap_err();
    assert_eq!(error.code.as_str(), "core.cancelled");
    for (rate, channels, start, duration) in [
        (0, 2, time(0), time(480)),
        (48_000, 0, time(0), time(480)),
        (48_000, 2, time(-1), time(480)),
        (48_000, 2, time(0), time(-1)),
        (48_000, 2, time(0), RationalTime::from_seconds(31)),
        (u32::MAX, u16::MAX, time(0), time(48_000)),
    ] {
        let error = call(rate, channels, start, duration, &|| false).unwrap_err();
        assert_eq!(error.code.as_str(), "core.invalid_argument");
    }
    let empty = call(48_000, 2, time(0), time(0), &|| false).unwrap();
    assert!(empty.samples.is_empty());
}

fn assert_samples(actual: &[f32], expected: &[f32], path: &Path) {
    assert_eq!(actual.len(), expected.len());
    for (i, (a, b)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(a.to_bits(), b.to_bits(), "{} sample {i}", path.display());
    }
}

#[test]
fn cancellation_during_source_setup_stops_before_delivering_pcm() {
    let Some(files) = fixtures() else { return };
    for path in files {
        let calls = std::cell::Cell::new(0);
        let cancel = || {
            let n = calls.get() + 1;
            calls.set(n);
            n >= 3
        };
        let error = decode_media_pcm_window(
            &item(path),
            Path::new("."),
            0,
            48_000,
            2,
            time(0),
            time(480),
            &cancel,
        )
        .unwrap_err();
        assert_eq!(error.code.as_str(), "core.cancelled");
        assert_eq!(calls.get(), 3);
    }
}

#[test]
fn windows_starting_beyond_eof_are_silent() {
    let Some(files) = fixtures() else { return };
    for path in files {
        for start in [96_000, 120_000] {
            let pcm = decode_media_pcm_window(
                &item(path),
                Path::new("."),
                0,
                48_000,
                2,
                time(start),
                time(480),
                &|| false,
            )
            .unwrap();
            assert_eq!(pcm.samples.len(), 960);
            assert!(pcm.samples.iter().all(|sample| *sample == 0.0));
        }
    }
}
