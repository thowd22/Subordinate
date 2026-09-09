//! The decode-ahead worker against the generated fixtures (TASK-17).
//!
//! What is checked here is that putting a decoder behind a bounded ring changes
//! nothing a caller can observe except when the frames were decoded: the same
//! presentation timestamps in the same order, the same frame after a seek, and
//! the same picture bytes. On top of that, the two properties the ring exists
//! for are asserted directly — the buffer never exceeds its capacity, and a
//! consumer that stops pulling makes the worker stall on backpressure rather
//! than run away with memory.
//!
//! Everything skips itself when the fixtures have not been generated, so
//! `cargo test` works on a fresh checkout.

use std::path::PathBuf;
use std::time::Duration;

use sub_media::{
    DecodeAhead, DecodeAheadOptions, Decoder, DecoderOptions, HardwarePreference, VideoFrame,
};
use sub_time::{Rational, RationalTime};

/// The fixture these tests read: 5 s of colour bars at 25 fps, one second GOP.
const FIXTURE: &str = "bars_1080p_h264.mp4";

/// How many frames that fixture holds.
const FRAME_COUNT: usize = 125;

/// The fixture path, or `None` when the fixtures were never generated.
fn fixture() -> Option<PathBuf> {
    let path = sub_test_support::try_fixture(FIXTURE);
    if path.is_none() {
        eprintln!("skipping: no fixture {FIXTURE}; run scripts/gen-fixtures.sh");
    }
    path
}

/// The fixture's frame rate.
fn rate() -> Rational {
    Rational::new(25, 1).expect("25 fps is a valid rate")
}

/// The timestamp of frame `index`, exactly.
fn frame_time(index: i64) -> RationalTime {
    RationalTime::new(index, rate())
}

/// Decoder options every test here shares: software decoding, so the pictures
/// are bit-for-bit identical between the two paths on every machine.
fn decoder_options() -> DecoderOptions {
    DecoderOptions {
        hardware: HardwarePreference::Software,
        frame_timeout: Duration::from_secs(20),
        ..DecoderOptions::default()
    }
}

/// The first plane of a frame, copied out so it outlives the frame.
fn luma(frame: &VideoFrame) -> Vec<u8> {
    frame
        .plane_data(0)
        .expect("every frame has a luma plane")
        .to_vec()
}

#[test]
fn decode_ahead_delivers_the_same_frames_as_a_plain_decoder() {
    let Some(path) = fixture() else { return };

    let mut plain = Decoder::open_with(&path, decoder_options()).expect("the fixture opens");
    let mut expected = Vec::new();
    while let Some(frame) = plain.next_frame().expect("the fixture decodes") {
        expected.push((frame.pts(), luma(&frame)));
    }
    assert_eq!(expected.len(), FRAME_COUNT, "fixture frame count");

    let mut ahead = DecodeAhead::open_with(
        &path,
        DecodeAheadOptions {
            capacity: 4,
            decoder: decoder_options(),
        },
    )
    .expect("the fixture opens for decode-ahead");

    let mut seen = 0usize;
    while let Some(frame) = ahead.next_frame().expect("the fixture decodes ahead") {
        let (pts, plane) = &expected[seen];
        assert_eq!(frame.pts(), *pts, "frame {seen} timestamp");
        assert_eq!(&luma(&frame), plane, "frame {seen} picture");
        assert!(
            ahead.occupancy() <= ahead.capacity(),
            "the ring must stay within its capacity"
        );
        seen += 1;
    }
    assert_eq!(
        seen,
        expected.len(),
        "every frame is delivered exactly once"
    );

    let stats = ahead.stats();
    assert_eq!(stats.capacity, 4);
    assert_eq!(
        stats.frames_delivered,
        u64::try_from(seen).expect("a small count")
    );
    assert_eq!(
        stats.frames_decoded,
        u64::try_from(seen).expect("a small count")
    );
    assert_eq!(stats.frames_dropped, 0, "nothing is dropped without a seek");
    assert!(
        stats
            .decode_time_mean()
            .is_some_and(|mean| mean > Duration::ZERO),
        "per-frame decode time is measured: {stats:?}"
    );
}

#[test]
fn a_paused_consumer_fills_the_ring_and_stalls_the_worker() {
    let Some(path) = fixture() else { return };

    let capacity = 3;
    let ahead = DecodeAhead::open_with(
        &path,
        DecodeAheadOptions {
            capacity,
            decoder: decoder_options(),
        },
    )
    .expect("the fixture opens for decode-ahead");

    // Nothing is pulled: the worker must fill the ring, then block on it.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while ahead.stats().backpressure_waits == 0 && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    let stats = ahead.stats();
    assert!(
        stats.backpressure_waits > 0,
        "the worker must have waited for space: {stats:?}"
    );
    assert_eq!(
        stats.occupancy, capacity,
        "a stalled worker leaves the ring exactly full"
    );
    assert_eq!(
        stats.frames_decoded,
        u64::try_from(capacity).expect("a small capacity"),
        "backpressure caps the work done, not just the memory held"
    );

    // And it stays capped: waiting longer decodes nothing more.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        ahead.stats().frames_decoded,
        u64::try_from(capacity).expect("a small capacity")
    );
}

#[test]
fn seeking_drops_the_buffer_and_resumes_at_the_target() {
    let Some(path) = fixture() else { return };

    let mut plain = Decoder::open_with(&path, decoder_options()).expect("the fixture opens");
    let target = frame_time(90);
    let wanted = plain
        .seek_to(target)
        .expect("the fixture seeks")
        .expect("frame 90 exists");
    let wanted = (wanted.pts(), luma(&wanted));
    // The frame after it, so the expectations never assume the container's
    // timestamps start at zero: this fixture's do not.
    let following = plain
        .next_frame()
        .expect("the fixture decodes on")
        .expect("a frame after the target")
        .pts();

    let mut ahead = DecodeAhead::open_with(
        &path,
        DecodeAheadOptions {
            capacity: 4,
            decoder: decoder_options(),
        },
    )
    .expect("the fixture opens for decode-ahead");

    // Let the worker run ahead, so the seek really has a buffer to throw away.
    let first = ahead
        .next_frame()
        .expect("the fixture decodes ahead")
        .expect("a first frame");
    assert!(
        first.pts() < wanted.0,
        "the decode starts at the head of the file, before the seek target"
    );
    drop(first);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while ahead.occupancy() == 0 && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    let buffered = ahead.occupancy();
    assert!(buffered > 0, "the worker must have decoded ahead");

    ahead.seek_to(target).expect("the seek is accepted");
    let frame = ahead
        .next_frame()
        .expect("the fixture seeks under decode-ahead")
        .expect("frame 90 exists");
    assert_eq!(frame.pts(), wanted.0, "the frame at the seek target");
    assert_eq!(luma(&frame), wanted.1, "the picture at the seek target");
    drop(frame);

    let stats = ahead.stats();
    assert_eq!(stats.seeks, 1, "one seek reached the decoder");
    assert!(
        stats.frames_dropped >= u64::try_from(buffered).expect("a small count"),
        "the buffered frames are dropped, not delivered: {stats:?}"
    );

    // Decoding continues in order from the target.
    let next = ahead
        .next_frame()
        .expect("decoding continues after the seek")
        .expect("frame 91 exists");
    assert_eq!(next.pts(), following);
}

#[test]
fn seeking_past_the_end_reports_end_of_stream() {
    let Some(path) = fixture() else { return };

    let mut ahead = DecodeAhead::open_with(
        &path,
        DecodeAheadOptions {
            capacity: 2,
            decoder: decoder_options(),
        },
    )
    .expect("the fixture opens for decode-ahead");

    ahead
        .seek_to(frame_time(
            i64::try_from(FRAME_COUNT).expect("a small count") + 100,
        ))
        .expect("the seek is accepted");
    assert!(
        ahead
            .next_frame()
            .expect("a seek past the end is not an error")
            .is_none(),
        "a target past the last frame ends the stream"
    );

    // And the handle is reusable: seeking back inside the file resumes it.
    ahead.seek_to(frame_time(10)).expect("the seek is accepted");
    let frame = ahead
        .next_frame()
        .expect("the fixture seeks back")
        .expect("frame 10 exists");
    assert_eq!(frame.pts(), frame_time(10));
}
