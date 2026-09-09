//! Thumbnail strips against the generated fixtures (TASK-25).
//!
//! These tests judge the job by what lands in the sidecar directory: a strip
//! of real JPEGs, one per requested picture, spread over the source, plus a
//! manifest that reads back. They then prove the two properties a background
//! job needs to be usable — that a second run does no decoding at all, and
//! that a run interrupted half way finishes the rest rather than starting
//! over.
//!
//! Everything skips itself when the fixtures have not been generated, so
//! `cargo test` works on a fresh checkout; CI runs `scripts/gen-fixtures.sh`
//! first, so there the assertions really execute.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use sub_core::{JobEvent, JobService, Priority};
use sub_media::{ThumbnailOptions, ThumbnailStrip, spawn_thumbnail_job};

/// The constant-rate fixture: 125 frames at 25 fps, five seconds of bars.
const CLIP: &str = "bars_1080p_h264.mp4";

/// Small strips keep these tests to a few seeks each.
fn options() -> ThumbnailOptions {
    ThumbnailOptions {
        count: 4,
        max_width: 160,
        quality: 70,
    }
}

/// A fixture path, or `None` when the fixtures were never generated.
fn fixture(name: &str) -> Option<PathBuf> {
    let path = sub_test_support::try_fixture(name);
    if path.is_none() {
        eprintln!("skipping: no fixture {name}; run scripts/gen-fixtures.sh");
    }
    path
}

/// A private sidecar directory for one test.
fn sidecar(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sub-thumbs-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a sidecar directory");
    dir
}

/// The bytes of a file that must be a JPEG.
fn jpeg_bytes(path: &std::path::Path) -> Vec<u8> {
    let bytes = std::fs::read(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    assert!(
        bytes.starts_with(&[0xFF, 0xD8]) && bytes.ends_with(&[0xFF, 0xD9]),
        "{} is not a complete JPEG",
        path.display()
    );
    bytes
}

#[test]
fn a_strip_of_n_compressed_pictures_lands_in_the_sidecar_directory() {
    let Some(clip) = fixture(CLIP) else { return };
    let dir = sidecar("strip");
    let options = options();

    let strip = ThumbnailStrip::generate(&clip, &dir, options)
        .unwrap_or_else(|err| panic!("[{}] {err}", err.code));

    assert_eq!(strip.frames().len(), options.count);
    assert_eq!(strip.options(), options);
    let mut previous = i64::MIN;
    for frame in strip.frames() {
        let bytes = jpeg_bytes(&frame.path(&dir));
        assert!(bytes.len() > 256, "a 160-wide picture is not 256 bytes");
        // 1920x1080 scaled to 160 wide is 160x90.
        assert_eq!((frame.width, frame.height), (160, 90));
        // The pictures are spread over the five seconds of the fixture, in
        // order, and none of them is the very first or very last frame.
        assert!(frame.pts_ns > previous, "timestamps must increase");
        previous = frame.pts_ns;
        assert!(frame.pts_ns > 0 && frame.pts_ns < 5_000_000_000);
    }

    // The strip reads back from the manifest alone.
    let loaded = ThumbnailStrip::load(&dir, strip.source_hash(), options).expect("a strip on disk");
    assert_eq!(loaded, strip);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_second_run_reuses_the_pictures_and_an_interrupted_one_finishes_the_rest() {
    let Some(clip) = fixture(CLIP) else { return };
    let dir = sidecar("resume");
    let options = options();

    let first = ThumbnailStrip::generate(&clip, &dir, options)
        .unwrap_or_else(|err| panic!("[{}] {err}", err.code));
    let written: Vec<Vec<u8>> = first
        .frames()
        .iter()
        .map(|frame| jpeg_bytes(&frame.path(&dir)))
        .collect();
    let modified = std::fs::metadata(first.frames()[0].path(&dir))
        .and_then(|meta| meta.modified())
        .expect("a modification time");

    // A second run finds the manifest and does not decode anything: the files
    // it would have written are untouched.
    let again = ThumbnailStrip::generate(&clip, &dir, options).expect("the strip again");
    assert_eq!(again, first);
    assert_eq!(
        std::fs::metadata(first.frames()[0].path(&dir))
            .and_then(|meta| meta.modified())
            .expect("a modification time"),
        modified,
        "a generated picture must not be written again"
    );

    // Now simulate a run that was interrupted after two pictures: throw away
    // the manifest and the last two files. Generation must produce exactly
    // those two again and leave the first two alone.
    std::fs::remove_file(dir.join(ThumbnailStrip::manifest_file_name(
        first.source_hash(),
        options,
    )))
    .expect("the manifest");
    for frame in &first.frames()[2..] {
        std::fs::remove_file(frame.path(&dir)).expect("a picture");
    }

    let resumed = ThumbnailStrip::generate(&clip, &dir, options).expect("the resumed strip");
    assert_eq!(resumed, first);
    for (frame, before) in first.frames().iter().zip(&written) {
        assert_eq!(
            &jpeg_bytes(&frame.path(&dir)),
            before,
            "picture {}",
            frame.index
        );
    }
    assert_eq!(
        std::fs::metadata(first.frames()[0].path(&dir))
            .and_then(|meta| meta.modified())
            .expect("a modification time"),
        modified,
        "an already generated picture must be skipped, not redone"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_job_service_runs_a_strip_off_the_calling_thread_and_reports_progress() {
    let Some(clip) = fixture(CLIP) else { return };
    let dir = sidecar("job");
    let options = options();

    let jobs = JobService::new(1);
    let events = jobs.subscribe();
    let job = spawn_thumbnail_job(&jobs, &clip, &dir, options, Priority::Interactive);
    let strip = job
        .wait()
        .unwrap_or_else(|err| panic!("[{}] {err}", err.code));
    assert_eq!(strip.frames().len(), options.count);

    let progress: Vec<(u64, u64)> = events
        .try_iter()
        .filter_map(|event| match event {
            JobEvent::Progress { done, total, .. } => Some((done, total)),
            _ => None,
        })
        .collect();
    assert_eq!(
        progress,
        (1..=options.count as u64)
            .map(|done| (done, options.count as u64))
            .collect::<Vec<_>>()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cancelled_thumbnail_job_stops_and_reports_cancellation() {
    let Some(clip) = fixture(CLIP) else { return };
    let dir = sidecar("cancel");
    // Enough pictures that cancelling lands in the middle of the work.
    let options = ThumbnailOptions {
        count: 64,
        max_width: 160,
        quality: 70,
    };

    let jobs = JobService::new(1);
    let job = spawn_thumbnail_job(&jobs, &clip, &dir, options, Priority::Background);
    let deadline = Instant::now() + Duration::from_secs(30);
    // Cancel as soon as the first picture is on disk, so the job is provably
    // in the middle of the strip rather than not started.
    let first = dir.join(ThumbnailStrip::frame_file_name(
        sub_model::ContentHash::of_file(&clip).expect("a hash"),
        options,
        0,
    ));
    while !first.is_file() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    job.cancel();

    let err = job.wait().expect_err("a cancelled job");
    assert_eq!(err.code.as_str(), "core.cancelled");
    assert!(
        ThumbnailStrip::load(
            &dir,
            sub_model::ContentHash::of_file(&clip).expect("a hash"),
            options
        )
        .is_none(),
        "a cancelled job must not leave a complete strip behind"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
