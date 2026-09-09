//! Frame-accurate seeking against the generated fixtures (TASK-15).
//!
//! A seek is only frame-accurate if the picture that comes back is the picture
//! that was asked for, so these tests judge it by the pixels rather than by the
//! timestamp the container reports. Two independent things are checked of every
//! frame a seek produces:
//!
//! * its **burnt-in timecode** names the right second. The fixture is colour
//!   bars with a timecode painted across the bottom by `timeoverlay`; the
//!   readable part of that overlay sits over the dark band of the pattern and
//!   is thresholded out of the luma plane here. The digits are learnt from a
//!   straight sequential decode, in which frame *n* must show the timecode of
//!   frame *n*, so the learning pass is itself an assertion: a burn-in that did
//!   not follow the expected timecode would fail while it ran.
//! * its **whole picture** is bit for bit the picture the same file decodes at
//!   that frame when it is decoded from the start. The 125 pictures of the
//!   fixture are all different from one another, so this pins the exact frame
//!   rather than merely the right second. Decoding is deterministic and every
//!   seek starts from a keyframe, so anything but an exact match is a wrong
//!   frame.
//!
//! The seconds digit is the finest field the overlay makes legible: the frames
//! counter is painted over the noise field at the right of the test pattern,
//! where a threshold cannot separate ink from picture. The picture comparison
//! is what carries the accuracy from a second down to a frame.
//!
//! Everything skips itself when the fixtures have not been generated, so
//! `cargo test` works on a fresh checkout; CI runs `scripts/gen-fixtures.sh`
//! first, so there the assertions really execute.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sub_media::{Decoder, DecoderOptions, VideoFrame};
use sub_time::{Rational, RationalTime, Timecode, TimecodeRate};

/// The fixture these tests read: 5 s of colour bars at 25 fps with a one
/// second GOP and a burnt-in timecode.
const FIXTURE: &str = "bars_1080p_h264.mp4";

/// How many frames that fixture holds.
const FRAME_COUNT: usize = 125;

/// How many random seek targets the sweep uses.
const RANDOM_TARGETS: usize = 50;

/// Luma above which a pixel counts as burn-in ink. The overlay is painted
/// white over the dark band of the bars, so the threshold sits nowhere near
/// either level and lossy compression cannot walk a pixel across it.
const INK: u8 = 200;

/// The fixture's frame rate.
fn rate() -> Rational {
    Rational::new(25, 1).expect("25 fps is a valid rate")
}

/// The fixture's timecode rate: 25 fps never drops frames.
fn timecode_rate() -> TimecodeRate {
    TimecodeRate::non_drop(rate()).expect("25 fps is a valid timecode rate")
}

/// The timecode frame `index` must carry.
fn timecode(index: usize) -> Timecode {
    Timecode::from_frame_number(
        i64::try_from(index).expect("a small index"),
        timecode_rate(),
    )
}

/// The fixture path, or `None` when the fixtures were never generated.
fn fixture() -> Option<PathBuf> {
    let path = sub_test_support::try_fixture(FIXTURE);
    if path.is_none() {
        eprintln!("skipping: no fixture {FIXTURE}; run scripts/gen-fixtures.sh");
    }
    path
}

/// Opens the fixture with the default options.
fn open(path: &Path) -> Decoder {
    Decoder::open(path).unwrap_or_else(|e| panic!("[{}] {e}", e.code))
}

/// The thresholded burn-in window of one frame.
///
/// The window is the legible half of the overlay: the lower band of the
/// picture, left of the noise field the test pattern puts under the frames
/// counter. What changes inside it over the clip is the timecode's seconds
/// digit.
struct BurnIn {
    /// Ink flags, row-major.
    ink: Vec<bool>,
}

impl BurnIn {
    /// Thresholds the burn-in window out of a frame's luma plane.
    fn of(frame: &VideoFrame) -> Self {
        let (w, h) = (frame.width() as usize, frame.height() as usize);
        let (x0, x1) = (w * 3 / 8, w * 3 / 4);
        let (y0, y1) = (h * 7 / 10, h * 9 / 10);
        let stride = frame.plane_stride(0).expect("a luma stride") as usize;
        let luma = frame.plane_data(0).expect("a luma plane");
        let mut ink = Vec::with_capacity((x1 - x0) * (y1 - y0));
        for y in y0..y1 {
            for x in x0..x1 {
                ink.push(luma[y * stride + x] > INK);
            }
        }
        Self { ink }
    }

    /// How many pixels two windows disagree on.
    fn differing(&self, other: &Self) -> usize {
        assert_eq!(self.ink.len(), other.ink.len(), "windows of one geometry");
        self.ink
            .iter()
            .zip(&other.ink)
            .filter(|(a, b)| a != b)
            .count()
    }

    /// How far two windows may disagree and still show the same digit: a
    /// thousandth of the window, which is room for the handful of pixels lossy
    /// compression moves and nowhere near a different digit.
    fn same_digit(&self) -> usize {
        self.ink.len() / 1000
    }

    /// How far apart two windows must be to show different digits: a
    /// hundredth of the window. The fixture's digits are far further apart
    /// than that, so no reading is ever decided by a near miss.
    fn different_digit(&self) -> usize {
        self.ink.len() / 100
    }
}

/// The fixture decoded from the start: what each frame's picture and burn-in
/// look like, and what the burn-in's digits mean.
struct Reference {
    /// Presentation timestamp of each frame, in stream order.
    pts: Vec<RationalTime>,
    /// A hash of each frame's luma plane, in stream order.
    picture: Vec<u64>,
    /// One burn-in window per timecode second, keyed by the second it shows.
    seconds: BTreeMap<u32, BurnIn>,
}

impl Reference {
    /// Decodes the whole fixture, learns its burn-in and checks both.
    fn build(path: &Path) -> Self {
        let mut decoder = open(path);
        let mut reference = Self {
            pts: Vec::new(),
            picture: Vec::new(),
            seconds: BTreeMap::new(),
        };
        let mut windows = Vec::new();
        while let Some(frame) = decoder
            .next_frame()
            .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
        {
            let index = reference.pts.len();
            reference.pts.push(frame.pts());
            reference.picture.push(picture_hash(&frame));
            let window = BurnIn::of(&frame);
            let second = timecode(index).seconds();
            if let Some(known) = reference.seconds.get(&second) {
                // Every frame of one second must paint the same digit: a
                // burn-in that did not follow the timecode fails here.
                let differing = known.differing(&window);
                assert!(
                    differing <= known.same_digit(),
                    "frame {index}: its burn-in differs from the rest of \
                     second {second} by {differing} pixels"
                );
                windows.push((index, window));
            } else {
                reference.seconds.insert(second, window);
            }
        }
        assert_eq!(
            reference.pts.len(),
            FRAME_COUNT,
            "the fixture is 125 frames long"
        );

        // Different seconds must look clearly different, or reading a digit
        // back would be guesswork.
        let digits: Vec<(&u32, &BurnIn)> = reference.seconds.iter().collect();
        assert_eq!(digits.len(), 5, "a five second clip shows five seconds");
        for (i, (second, window)) in digits.iter().enumerate() {
            for (other, other_window) in &digits[i + 1..] {
                let differing = window.differing(other_window);
                assert!(
                    differing >= window.different_digit(),
                    "seconds {second} and {other} are only {differing} pixels apart"
                );
            }
        }

        // Every frame of the sequential decode must read back as its own
        // second, and carry a picture no other frame carries.
        for (index, window) in &windows {
            assert_eq!(
                reference.read_second(window),
                timecode(*index).seconds(),
                "frame {index} of a sequential decode"
            );
        }
        let mut hashes = reference.picture.clone();
        hashes.sort_unstable();
        let total = hashes.len();
        hashes.dedup();
        assert_eq!(
            hashes.len(),
            total,
            "every frame of the fixture is a different picture"
        );
        reference
    }

    /// The timecode second a burn-in window shows.
    ///
    /// # Panics
    ///
    /// Panics when the window matches no learnt digit clearly enough to be
    /// read, which means the picture is not this fixture's burn-in at all.
    fn read_second(&self, window: &BurnIn) -> u32 {
        let mut scored: Vec<(usize, u32)> = self
            .seconds
            .iter()
            .map(|(second, learnt)| (learnt.differing(window), *second))
            .collect();
        scored.sort_unstable();
        let (best, second) = scored[0];
        let (runner_up, _) = scored[1];
        assert!(
            best <= window.same_digit() && runner_up >= window.different_digit(),
            "the burn-in could not be read: best '{second}' at {best} pixels, \
             next at {runner_up}"
        );
        second
    }

    /// Asserts that `frame` is the fixture's frame number `index`: its burnt-in
    /// timecode names the right second, and its picture is that frame's picture
    /// exactly.
    fn assert_is_frame(&self, frame: &VideoFrame, index: usize, what: &str) {
        let expected = timecode(index);
        assert_eq!(
            self.read_second(&BurnIn::of(frame)),
            expected.seconds(),
            "{what}: the burnt-in timecode must read {expected}"
        );
        assert_eq!(
            picture_hash(frame),
            self.picture[index],
            "{what}: the picture must be frame {index} of the fixture"
        );
        assert_eq!(
            frame.pts().value(),
            self.pts[index].value(),
            "{what}: frame {index} has one timestamp"
        );
    }
}

/// FNV-1a over a frame's luma plane, row by row so the padding a stride leaves
/// at the end of a row is never hashed.
fn picture_hash(frame: &VideoFrame) -> u64 {
    let stride = frame.plane_stride(0).expect("a luma stride") as usize;
    let width = frame.width() as usize;
    let luma = frame.plane_data(0).expect("a luma plane");
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for row in 0..frame.height() as usize {
        for byte in &luma[row * stride..row * stride + width] {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// A deterministic xorshift, so the "random" targets are the same on every
/// machine and a failure can be reproduced.
struct Rng(u64);

impl Rng {
    /// The next value below `bound`.
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % bound as u64).expect("a small bound")
    }
}

/// One nanosecond: the smallest step a seek target can be nudged by.
fn one_nanosecond() -> RationalTime {
    RationalTime::new(1, sub_media::probe::NANOSECONDS)
}

#[test]
fn fifty_random_seeks_land_on_the_frame_that_was_asked_for() {
    let Some(path) = fixture() else {
        return;
    };
    let reference = Reference::build(&path);
    let mut decoder = open(&path);
    let mut rng = Rng(0x2f6e_2b1c_9a35_11d7);

    // The last frame is in the sweep by name: it is what a scrub to the end of
    // a clip asks for, and the frame a keyframe seek is most likely to
    // undershoot.
    let mut targets: Vec<usize> = vec![FRAME_COUNT - 1, 0];
    while targets.len() < RANDOM_TARGETS {
        targets.push(rng.below(FRAME_COUNT));
    }

    for (attempt, &index) in targets.iter().enumerate() {
        // Half the targets ask for the frame's own timestamp; the others ask
        // for an instant just inside the previous frame's interval, which is
        // what a scrub bar produces. Both must land on the same frame.
        let target = if attempt % 2 == 0 || index == 0 {
            reference.pts[index]
        } else {
            reference.pts[index - 1] + one_nanosecond()
        };
        let frame = decoder
            .seek_to(target)
            .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
            .unwrap_or_else(|| panic!("seek {attempt} to frame {index} found no frame"));
        reference.assert_is_frame(&frame, index, &format!("seek {attempt} to frame {index}"));
    }
}

#[test]
fn a_seek_lands_on_the_first_frame_at_or_after_the_target() {
    let Some(path) = fixture() else {
        return;
    };
    let reference = Reference::build(&path);
    let mut decoder = open(&path);

    // An instant just inside frame 60's interval belongs to frame 61: frame 60
    // starts before the target, frame 61 does not.
    let target = reference.pts[60] + one_nanosecond();
    let frame = decoder
        .seek_to(target)
        .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
        .expect("frame 61 exists");
    assert!(frame.pts() >= target, "the frame is at or after the target");
    assert!(reference.pts[60] < target, "and the one before it is not");
    reference.assert_is_frame(&frame, 61, "a target inside frame 60's interval");
    assert_eq!(decoder.seek_count(), 1, "one flushing keyframe seek");
    assert_eq!(decoder.position(), Some(frame.pts()));
}

#[test]
fn seeking_forward_inside_the_gop_does_not_re_seek() {
    let Some(path) = fixture() else {
        return;
    };
    let reference = Reference::build(&path);
    let mut decoder = open(&path);

    let frame = decoder
        .seek_to(reference.pts[10])
        .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
        .expect("frame 10 exists");
    reference.assert_is_frame(&frame, 10, "the first seek");
    let after_first = decoder.seek_count();
    assert_eq!(after_first, 1, "the first seek always seeks");

    // Frames 11 to 24 are in the fixture's one-second GOP, which the decoder is
    // already inside: seeking again would decode from the keyframe it has just
    // passed, so it must not.
    for index in [11_usize, 20, 24] {
        let frame = decoder
            .seek_to(reference.pts[index])
            .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
            .unwrap_or_else(|| panic!("frame {index} exists"));
        reference.assert_is_frame(&frame, index, "a forward seek inside the GOP");
        assert_eq!(
            decoder.seek_count(),
            after_first,
            "reaching frame {index} forward must not issue a seek"
        );
    }

    // Backwards, and far forwards, both have to seek.
    let frame = decoder
        .seek_to(reference.pts[5])
        .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
        .expect("frame 5 exists");
    reference.assert_is_frame(&frame, 5, "a backward seek");
    assert_eq!(decoder.seek_count(), after_first + 1, "backwards re-seeks");

    let frame = decoder
        .seek_to(reference.pts[124])
        .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
        .expect("the last frame exists");
    reference.assert_is_frame(&frame, 124, "a seek past the window");
    assert_eq!(
        decoder.seek_count(),
        after_first + 2,
        "a target further ahead than the window re-seeks"
    );
}

#[test]
fn a_narrow_forward_window_makes_every_seek_a_real_seek() {
    let Some(path) = fixture() else {
        return;
    };
    let reference = Reference::build(&path);
    let options = DecoderOptions {
        // Narrower than one frame, so nothing is reachable by decoding forward.
        forward_decode_window: Duration::from_millis(1),
        ..DecoderOptions::default()
    };
    let mut decoder =
        Decoder::open_with(&path, options).unwrap_or_else(|e| panic!("[{}] {e}", e.code));
    for (seeks, index) in [11_usize, 12, 13].into_iter().enumerate() {
        let frame = decoder
            .seek_to(reference.pts[index])
            .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
            .unwrap_or_else(|| panic!("frame {index} exists"));
        reference.assert_is_frame(&frame, index, "a seek with no forward window");
        assert_eq!(
            decoder.seek_count(),
            seeks as u64 + 1,
            "each target re-seeks"
        );
    }
}

#[test]
fn seeking_past_the_end_reports_the_end_and_seeking_back_recovers() {
    let Some(path) = fixture() else {
        return;
    };
    let reference = Reference::build(&path);
    let mut decoder = open(&path);

    assert!(
        decoder
            .seek_to(RationalTime::from_seconds(60))
            .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
            .is_none(),
        "there is no frame a minute into a five second clip"
    );

    // The end of a stream is not the end of the decoder: a flushing seek brings
    // it back, which is what scrubbing away from the tail needs.
    let frame = decoder
        .seek_to(reference.pts[7])
        .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
        .expect("frame 7 exists");
    reference.assert_is_frame(&frame, 7, "a seek back from the end");
}

#[test]
fn a_negative_target_is_the_start_of_the_stream() {
    let Some(path) = fixture() else {
        return;
    };
    let reference = Reference::build(&path);
    let mut decoder = open(&path);
    let frame = decoder
        .seek_to(RationalTime::from_seconds(-3))
        .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
        .expect("the first frame exists");
    reference.assert_is_frame(&frame, 0, "a negative target");
}

#[test]
fn a_target_counted_in_frames_needs_no_conversion_by_the_caller() {
    let Some(path) = fixture() else {
        return;
    };
    let reference = Reference::build(&path);
    let mut decoder = open(&path);
    // The fixture's timestamps start 80 ms in, so frame 37 is asked for as 37
    // frames at 25 fps from that start: exact rational arithmetic, compared
    // against nanosecond timestamps with no float and no tolerance anywhere.
    let target = reference.pts[0] + RationalTime::from_frames(37, rate());
    let frame = decoder
        .seek_to(target)
        .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
        .expect("frame 37 exists");
    reference.assert_is_frame(&frame, 37, "a target expressed in frames");
}
