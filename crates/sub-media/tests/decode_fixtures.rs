//! Decodes the generated fixtures through [`sub_media::Decoder`].
//!
//! Skips itself when the fixtures have not been generated, so `cargo test`
//! still works on a fresh checkout; CI runs `scripts/gen-fixtures.sh` first, so
//! there the assertions really execute. CI has no GPU, so these decodes go
//! through a software decoder; the hardware preference only changes which
//! element is plugged, never the frames or their timestamps.

use std::path::PathBuf;
use std::time::Duration;

use sub_media::{Decoder, DecoderOptions, FrameFormat, HardwarePreference};
use sub_time::RationalTime;

/// How many frames the timing assertions look at.
const FRAMES: usize = 60;

/// Turns on the decoder's own logging once per test binary, so a failure
/// reports which element was plugged.
fn log_decoder_choice() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = sub_core::logging::init("sub_media=debug");
    });
}

/// The 1080p fixture, or `None` when the fixtures were never generated.
fn fixture(name: &str) -> Option<PathBuf> {
    log_decoder_choice();
    let path = sub_test_support::try_fixture(name);
    if path.is_none() {
        eprintln!("skipping: no fixture {name}; run scripts/gen-fixtures.sh");
    }
    path
}

/// Pulls up to `count` frames, returning them in the order they arrived.
fn take_frames(decoder: &mut Decoder, count: usize) -> Vec<sub_media::VideoFrame> {
    let mut frames = Vec::with_capacity(count);
    while frames.len() < count {
        match decoder.next_frame() {
            Ok(Some(frame)) => frames.push(frame),
            Ok(None) => break,
            Err(e) => panic!("[{}] {e}", e.code),
        }
    }
    frames
}

#[test]
fn the_first_frames_of_the_1080p_fixture_decode_with_monotonic_timestamps() {
    let Some(path) = fixture("bars_1080p_h264.mp4") else {
        return;
    };
    let mut decoder = Decoder::open(&path).unwrap_or_else(|e| panic!("[{}] {e}", e.code));
    let frames = take_frames(&mut decoder, FRAMES);

    assert_eq!(
        frames.len(),
        FRAMES,
        "the fixture is longer than {FRAMES} frames"
    );
    assert!(
        decoder.decoder_element().is_some(),
        "the plugged decoder element must be reported once frames flow"
    );

    let mut previous: Option<RationalTime> = None;
    for (index, frame) in frames.iter().enumerate() {
        assert_eq!(frame.width(), 1920, "frame {index}: width");
        assert_eq!(frame.height(), 1080, "frame {index}: height");
        assert_eq!(
            frame.format(),
            FrameFormat::Nv12,
            "frame {index}: the fixture is 8-bit 4:2:0, so NV12 is reachable"
        );
        assert_eq!(
            frame.pts().rate(),
            sub_media::probe::NANOSECONDS,
            "frame {index}: timestamps are exact nanoseconds"
        );
        assert!(!frame.pts().is_negative(), "frame {index}: negative PTS");
        if let Some(previous) = previous {
            assert!(
                frame.pts().value() > previous.value(),
                "frame {index}: PTS {} is not after {}",
                frame.pts().value(),
                previous.value()
            );
        }
        previous = Some(frame.pts());

        // NV12 is luma then interleaved chroma, each with its own stride.
        let luma = frame.plane_data(0).expect("luma plane");
        let stride = frame.plane_stride(0).expect("luma stride") as usize;
        assert!(stride >= 1920, "frame {index}: luma stride {stride}");
        assert!(
            luma.len() >= stride * 1080,
            "frame {index}: luma plane is short"
        );
        assert!(frame.plane_data(1).is_some(), "frame {index}: chroma plane");
        assert!(
            frame.plane_data(2).is_none(),
            "frame {index}: NV12 has two planes"
        );
    }
}

#[test]
fn frame_spacing_matches_the_fixtures_constant_frame_rate() {
    let Some(path) = fixture("bars_1080p_h264.mp4") else {
        return;
    };
    let mut decoder = Decoder::open(&path).unwrap_or_else(|e| panic!("[{}] {e}", e.code));
    let frames = take_frames(&mut decoder, FRAMES);
    assert!(frames.len() >= 3, "too few frames to judge spacing");

    // The fixture is 25 fps, so every gap is exactly 40 ms in nanoseconds.
    let expected = 40_000_000_i64;
    for pair in frames.windows(2) {
        let gap = pair[1].pts().value() - pair[0].pts().value();
        assert_eq!(gap, expected, "constant frame rate means an exact gap");
    }
}

#[test]
fn the_documented_fallback_format_decodes_the_same_frames() {
    let Some(path) = fixture("bars_1080p_h264.mp4") else {
        return;
    };
    let options = DecoderOptions {
        format: FrameFormat::I420,
        ..DecoderOptions::default()
    };
    let mut decoder =
        Decoder::open_with(&path, options).unwrap_or_else(|e| panic!("[{}] {e}", e.code));
    let frames = take_frames(&mut decoder, 5);
    assert_eq!(frames.len(), 5, "five I420 frames");
    for frame in &frames {
        assert_eq!(frame.format(), FrameFormat::I420);
        assert_eq!(frame.width(), 1920);
        // I420 keeps its two chroma planes apart, where NV12 interleaves them.
        assert!(frame.plane_data(2).is_some(), "I420 has three planes");
        assert!(frame.plane_data(3).is_none(), "and no more than three");
    }
}

#[test]
fn a_software_decode_matches_the_hardware_preferred_one() {
    let Some(path) = fixture("bars_1080p_h264.mp4") else {
        return;
    };
    let options = DecoderOptions {
        hardware: HardwarePreference::Software,
        ..DecoderOptions::default()
    };
    let mut preferred = Decoder::open(&path).unwrap_or_else(|e| panic!("[{}] {e}", e.code));
    let mut plain =
        Decoder::open_with(&path, options).unwrap_or_else(|e| panic!("[{}] {e}", e.code));

    let a: Vec<i64> = take_frames(&mut preferred, FRAMES)
        .iter()
        .map(|f| f.pts().value())
        .collect();
    let b: Vec<i64> = take_frames(&mut plain, FRAMES)
        .iter()
        .map(|f| f.pts().value())
        .collect();
    assert_eq!(a, b, "the decoder choice must not change the timestamps");
}

#[test]
fn dropping_a_decoder_mid_stream_tears_the_pipeline_down() {
    let Some(path) = fixture("bars_1080p_h264.mp4") else {
        return;
    };
    // Opening, decoding a little and dropping repeatedly would leak decoders,
    // pipelines or file handles if teardown were incomplete; a leak shows up
    // here as an open failure once the process runs out of them.
    for _ in 0..8 {
        let mut decoder = Decoder::open(&path).unwrap_or_else(|e| panic!("[{}] {e}", e.code));
        let frames = take_frames(&mut decoder, 3);
        assert_eq!(frames.len(), 3, "three frames before the drop");
        drop(decoder);
    }
}

#[test]
fn decoding_past_the_end_of_the_stream_reports_end_rather_than_an_error() {
    let Some(path) = fixture("bars_1080p_h264.mp4") else {
        return;
    };
    let mut decoder = Decoder::open(&path).unwrap_or_else(|e| panic!("[{}] {e}", e.code));
    let mut seen = 0_usize;
    while let Some(_frame) = decoder
        .next_frame()
        .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
    {
        seen += 1;
        assert!(seen < 100_000, "the fixture must end");
    }
    assert_eq!(seen, 125, "the 5 s, 25 fps fixture holds 125 frames");
    assert!(
        decoder
            .next_frame()
            .expect("end of stream is not an error")
            .is_none(),
        "past the end stays at the end"
    );
}

#[test]
fn an_audio_only_file_decodes_no_video_frames() {
    let Some(path) = fixture("tone_48k_stereo.wav") else {
        return;
    };
    let mut decoder = Decoder::open(&path).unwrap_or_else(|e| panic!("[{}] {e}", e.code));
    let err = decoder.next_frame().expect_err("no video stream to decode");
    assert_eq!(err.code.as_str(), "media.no_video_stream");
}

#[test]
fn a_missing_file_is_reported_as_unreadable() {
    let err = Decoder::open(&PathBuf::from("/definitely/not/here.mp4")).expect_err("must fail");
    assert_eq!(err.code.as_str(), "media.file_unreadable");
}

#[test]
fn a_corrupt_file_fails_with_a_media_code() {
    let dir = std::env::temp_dir().join(format!("sub-media-decode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let path = dir.join("corrupt.mp4");
    let mut bytes = b"\x00\x00\x00\x18ftypisom\x00\x00\x02\x00isomiso2".to_vec();
    bytes.extend(std::iter::repeat_n(0xA5_u8, 4096));
    std::fs::write(&path, &bytes).expect("write corrupt fixture");

    let options = DecoderOptions {
        frame_timeout: Duration::from_secs(5),
        ..DecoderOptions::default()
    };
    match Decoder::open_with(&path, options) {
        // The failure can appear when the pipeline starts or when the first
        // frame is asked for; either way it is a media-domain error, never a
        // hang and never a frame.
        Ok(mut decoder) => {
            let err = decoder
                .next_frame()
                .expect_err("a corrupt file has no frames");
            assert_eq!(err.code.domain(), "media");
        }
        Err(err) => assert_eq!(err.code.domain(), "media"),
    }

    std::fs::remove_dir_all(&dir).expect("clean up");
}
