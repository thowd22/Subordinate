//! The PTS index against the generated fixtures (TASK-16).
//!
//! `vfr_60_30.mkv` is three seconds at 30 fps followed by three at 60 fps, so
//! no single frame duration describes it: a seek that multiplied a frame
//! number by one duration would land on the wrong picture in the second half
//! of the file. These tests build the index from that file, check it really
//! records two different frame durations, and judge index-driven seeking and
//! stepping by the pictures they produce — a frame reached by a seek must be
//! bit for bit the picture the same file decodes at that frame number when it
//! is decoded from the start.
//!
//! The seek and stepping assertions cover the whole of both fixtures,
//! including the frame-rate change in the middle of the variable one.
//!
//! Everything skips itself when the fixtures have not been generated, so
//! `cargo test` works on a fresh checkout; CI runs `scripts/gen-fixtures.sh`
//! first, so there the assertions really execute.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sub_media::{
    CancelToken, Decoder, IndexJob, IndexedDecoder, LazyPtsIndex, PtsIndex, VideoFrame,
};

/// The variable-frame-rate fixture.
const VFR: &str = "vfr_60_30.mkv";

/// The constant-rate fixture, used as the control: 125 frames at 25 fps.
const CFR: &str = "bars_1080p_h264.mp4";

/// The frame the VFR fixture changes rate at: 90 frames at 30 fps come before
/// it, 180 frames at 60 fps from it onwards.
const VFR_RATE_CHANGE: usize = 90;

/// A fixture path, or `None` when the fixtures were never generated.
fn fixture(name: &str) -> Option<PathBuf> {
    let path = sub_test_support::try_fixture(name);
    if path.is_none() {
        eprintln!("skipping: no fixture {name}; run scripts/gen-fixtures.sh");
    }
    path
}

/// Builds the index of a fixture, failing with its error code.
fn index_of(path: &Path) -> PtsIndex {
    PtsIndex::build(path).unwrap_or_else(|e| panic!("[{}] {e}", e.code))
}

/// A cheap fingerprint of a decoded picture: every plane's bytes folded row by
/// row, so the padding in a stride never joins the value.
fn fingerprint(frame: &VideoFrame) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let planes = u32::try_from(frame.format().plane_count()).expect("a small plane count");
    for plane in 0..planes {
        let (Some(data), Some(stride)) = (frame.plane_data(plane), frame.plane_stride(plane))
        else {
            continue;
        };
        let width = frame.width() as usize;
        for row in data.chunks(stride as usize) {
            for byte in row.iter().take(width) {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x1000_0000_01b3);
            }
        }
    }
    hash
}

/// The fixture decoded straight through: one fingerprint per frame, in order.
fn reference(path: &Path) -> Vec<u64> {
    let mut decoder = Decoder::open(path).unwrap_or_else(|e| panic!("[{}] {e}", e.code));
    let mut frames = Vec::new();
    while let Some(frame) = decoder
        .next_frame()
        .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
    {
        frames.push(fingerprint(&frame));
    }
    frames
}

#[test]
fn index_records_more_than_one_frame_duration_for_a_vfr_source() {
    let Some(path) = fixture(VFR) else { return };
    let index = index_of(&path);

    assert!(index.len() > 100, "the fixture holds many frames");
    assert!(!index.is_empty());
    assert_eq!(index.is_keyframe(0), Some(true), "frame zero is a keyframe");
    assert_eq!(index.pts(0), Some(sub_time::RationalTime::new(0, rate())));

    // Presentation order: entry n never precedes entry n - 1.
    for frame in 1..index.len() {
        let previous = index.pts(frame - 1).expect("an earlier frame");
        let current = index.pts(frame).expect("this frame");
        assert!(
            current >= previous,
            "frame {frame} must not precede frame {}",
            frame - 1
        );
    }

    // The point of the index: the file genuinely has more than one frame
    // duration, so nothing may assume a constant one.
    let mut durations: Vec<i64> = (0..index.len() - 1)
        .filter_map(|frame| index.duration_of(frame))
        .map(sub_time::RationalTime::value)
        .collect();
    durations.sort_unstable();
    durations.dedup();
    assert!(
        durations.len() > 1,
        "the VFR fixture must show more than one frame duration, saw {durations:?}"
    );
    // Both the 30 fps and the 60 fps spacing are in there, within the
    // millisecond the container rounds its timestamps to.
    let near = |wanted: i64| {
        durations
            .iter()
            .any(|d| (d - wanted).abs() < 1_000_000)
            .then_some(wanted)
    };
    assert_eq!(near(33_333_333), Some(33_333_333), "a 30 fps spacing");
    assert_eq!(near(16_666_666), Some(16_666_666), "a 60 fps spacing");

    // Every frame is reachable from a keyframe at or before it.
    for frame in 0..index.len() {
        let keyframe = index
            .keyframe_at_or_before(frame)
            .unwrap_or_else(|| panic!("frame {frame} has no keyframe before it"));
        assert!(keyframe <= frame);
        assert_eq!(index.is_keyframe(keyframe), Some(true));
    }
    assert_eq!(index.keyframe_at_or_before(index.len()), None);
}

/// The nanosecond rate index timestamps are expressed at.
fn rate() -> sub_time::Rational {
    sub_time::Rational::new(1_000_000_000, 1).expect("a nanosecond rate")
}

#[test]
fn the_vfr_fixture_gives_every_picture_its_own_instant() {
    // A guard on the fixture itself: seeking can only name a picture by time
    // if no two pictures share an instant and none arrives before the one
    // before it. An encoder that assumes a single frame duration across the
    // rate change breaks exactly this, so the generator is checked here.
    let Some(path) = fixture(VFR) else { return };
    let index = index_of(&path);
    assert_eq!(index.len(), 270, "90 frames at 30 fps then 180 at 60 fps");

    for frame in 1..index.len() {
        let previous = index.pts(frame - 1).expect("an earlier frame");
        let current = index.pts(frame).expect("this frame");
        assert!(
            current > previous,
            "frame {frame} at {current:?} must come after frame {} at {previous:?}",
            frame - 1
        );
    }

    // And the two halves really do run at the two rates, either side of the
    // one frame where the spacing changes.
    let spacing = |frame: usize| {
        index
            .duration_of(frame)
            .map(sub_time::RationalTime::value)
            .expect("a frame duration")
    };
    let near = |value: i64, wanted: i64| (value - wanted).abs() < 1_000_000;
    assert!(near(spacing(0), 33_333_333), "{} ns", spacing(0));
    assert!(
        near(spacing(VFR_RATE_CHANGE - 2), 33_333_333),
        "{} ns",
        spacing(VFR_RATE_CHANGE - 2)
    );
    assert!(
        near(spacing(VFR_RATE_CHANGE), 16_666_666),
        "{} ns",
        spacing(VFR_RATE_CHANGE)
    );
    assert!(
        near(spacing(index.len() - 2), 16_666_666),
        "{} ns",
        spacing(index.len() - 2)
    );
}

#[test]
fn lookup_by_time_round_trips_through_every_frame() {
    for name in [VFR, CFR] {
        let Some(path) = fixture(name) else { continue };
        let index = index_of(&path);
        for frame in 0..index.len() {
            let pts = index.pts(frame).expect("a frame");
            // Every picture has its own instant, so a lookup by that instant
            // answers with that very frame number.
            let found = index
                .frame_at(pts)
                .unwrap_or_else(|| panic!("{name}: frame {frame} is not found by its own time"));
            assert_eq!(found, frame, "{name}: frame {frame} by its own time");
            let after = index.frame_at_or_after(pts).expect("a frame at or after");
            assert_eq!(after, frame, "{name}: frame {frame} at or after its time");
        }
        // Before the first picture there is nothing, and past the last one
        // there is nothing after.
        let start = index.pts(0).expect("a first frame");
        assert_eq!(
            index.frame_at(start.checked_sub(one_nanosecond()).expect("a time")),
            None
        );
        let end = index.pts(index.len() - 1).expect("a last frame");
        assert_eq!(
            index.frame_at_or_after(end.checked_add(one_nanosecond()).expect("a time")),
            None
        );
    }
}

/// One nanosecond, for the edges of a lookup.
fn one_nanosecond() -> sub_time::RationalTime {
    sub_time::RationalTime::new(1, rate())
}

#[test]
fn index_driven_seek_and_stepping_land_on_the_correct_frame() {
    // The constant-rate fixture is the control: the whole file is covered.
    if let Some(path) = fixture(CFR) {
        let reference = reference(&path);
        let targets: Vec<usize> = vec![0, 1, 24, 25, 26, 62, 99, 124, 7, 100];
        assert_seeks_land(&path, &reference, &targets, 50, 12);
    }

    // The VFR fixture, over the whole file: before the rate change, on the
    // first frame of the faster half, and well past it, where a seek that
    // assumed one frame duration would be a hundred frames out.
    let Some(path) = fixture(VFR) else { return };
    let reference = reference(&path);
    let last = reference.len() - 1;
    let targets: Vec<usize> = vec![
        0,
        1,
        29,
        VFR_RATE_CHANGE - 1,
        VFR_RATE_CHANGE,
        VFR_RATE_CHANGE + 1,
        150,
        last,
        12,
        200,
    ];
    // Stepping starts one frame before the rate change, so the steps walk
    // across it and every 16.6 ms frame after it is checked against the
    // picture a straight decode produces.
    assert_seeks_land(&path, &reference, &targets, VFR_RATE_CHANGE - 1, 12);
}

/// Seeks to each of `targets` and steps `steps` frames on from `step_start`,
/// checking every picture against `reference`.
fn assert_seeks_land(
    path: &Path,
    reference: &[u64],
    targets: &[usize],
    step_start: usize,
    steps: usize,
) {
    let index = Arc::new(index_of(path));
    assert_eq!(
        index.len(),
        reference.len(),
        "{}: the index must hold one entry per decoded picture",
        path.display()
    );

    let mut decoder = IndexedDecoder::open(path, Arc::clone(&index))
        .unwrap_or_else(|e| panic!("[{}] {e}", e.code));
    assert_eq!(decoder.frame_count(), reference.len());
    assert_eq!(decoder.current_frame(), None);

    for &target in targets {
        let frame = decoder
            .seek_to_frame(target)
            .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
            .unwrap_or_else(|| panic!("frame {target} must decode"));
        assert_eq!(
            frame.pts(),
            index.pts(target).expect("an indexed frame"),
            "seek to frame {target} landed on another timestamp"
        );
        assert_eq!(decoder.current_frame(), Some(target));
        assert_eq!(
            fingerprint(&frame),
            reference[target],
            "seek to frame {target} produced another picture"
        );
    }

    // Past the end there is no frame.
    assert!(
        decoder
            .seek_to_frame(reference.len())
            .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
            .is_none()
    );

    // Stepping forward one frame at a time follows the index's own spacing.
    decoder
        .seek_to_frame(step_start)
        .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
        .expect("the step start decodes");
    for (expected, wanted) in reference
        .iter()
        .enumerate()
        .take(step_start + steps + 1)
        .skip(step_start + 1)
    {
        let frame = decoder
            .step(1)
            .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
            .unwrap_or_else(|| panic!("frame {expected} must decode"));
        assert_eq!(decoder.current_frame(), Some(expected));
        assert_eq!(
            fingerprint(&frame),
            *wanted,
            "stepping to frame {expected} produced another picture"
        );
    }

    // And stepping backwards lands exactly as well.
    let expected = step_start + steps - 4;
    let frame = decoder
        .step(-4)
        .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
        .expect("a frame four back");
    assert_eq!(decoder.current_frame(), Some(expected));
    assert_eq!(fingerprint(&frame), reference[expected]);

    // Stepping before the first frame clamps to it.
    let frame = decoder
        .step(-10_000)
        .unwrap_or_else(|e| panic!("[{}] {e}", e.code))
        .expect("the first frame");
    assert_eq!(decoder.current_frame(), Some(0));
    assert_eq!(fingerprint(&frame), reference[0]);
}

#[test]
fn the_index_is_built_lazily_and_cached_in_the_sidecar_dir() {
    let Some(path) = fixture(VFR) else { return };
    let cache_dir =
        std::env::temp_dir().join(format!("sub-media-index-cache-{}-lazy", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache_dir);

    let lazy = LazyPtsIndex::new(path.clone(), Some(cache_dir.clone()));
    assert!(
        !lazy.is_built(),
        "nothing is parsed before the first access"
    );
    let built = lazy.get().unwrap_or_else(|e| panic!("[{}] {e}", e.code));
    assert!(
        lazy.is_built(),
        "the index is memoised after the first access"
    );
    assert!(Arc::ptr_eq(
        &built,
        &lazy.get().unwrap_or_else(|e| panic!("[{}] {e}", e.code))
    ));

    let cache_file = cache_dir.join(PtsIndex::cache_file_name(built.source_hash()));
    assert!(
        cache_file.is_file(),
        "the built index is cached at {}",
        cache_file.display()
    );

    // A fresh lazy index over the same file reads that cache rather than
    // parsing again, and gets exactly the same index back.
    let reread = LazyPtsIndex::new(path.clone(), Some(cache_dir.clone()))
        .get()
        .unwrap_or_else(|e| panic!("[{}] {e}", e.code));
    assert_eq!(*reread, *built);

    // A corrupt cache is rebuilt, not fatal.
    std::fs::write(&cache_file, "{ not an index").expect("cache is corrupted");
    let rebuilt = LazyPtsIndex::new(path, Some(cache_dir.clone()))
        .get()
        .unwrap_or_else(|e| panic!("[{}] {e}", e.code));
    assert_eq!(*rebuilt, *built);

    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn an_index_build_is_a_cancellable_background_job() {
    let Some(path) = fixture(VFR) else { return };

    // A token that is already set stops the build before it parses anything.
    let cancel = CancelToken::new();
    cancel.cancel();
    let error = PtsIndex::build_cancellable(&path, &cancel).expect_err("a cancelled build fails");
    assert_eq!(error.code.as_str(), "core.cancelled");

    // A job runs on its own thread and stops soon after it is cancelled.
    let job = IndexJob::spawn(path.clone(), None);
    job.cancel();
    let started = Instant::now();
    let outcome = job.join();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "a cancelled job must stop promptly"
    );
    match outcome {
        // Either it was stopped, or it had already finished the parse.
        Err(err) => assert_eq!(err.code.as_str(), "core.cancelled"),
        Ok(index) => assert!(!index.is_empty()),
    }

    // A job that is left to run produces the same index as a direct build.
    let job = IndexJob::spawn(path.clone(), None);
    let index = job.join().unwrap_or_else(|e| panic!("[{}] {e}", e.code));
    assert_eq!(*index, index_of(&path));
}
