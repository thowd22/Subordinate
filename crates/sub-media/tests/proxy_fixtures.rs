//! Proxy generation against the generated fixtures (TASK-69).
//!
//! These tests judge a proxy by the file that lands in the sidecar directory:
//! a real intra-only movie at the requested size, holding exactly the frames
//! the source holds, at the timestamps the source has them at. They then prove
//! the two properties a background job needs — that a finished proxy is reused
//! rather than made again, and that a cancelled one leaves nothing behind.
//!
//! Everything skips itself when the fixtures have not been generated, so
//! `cargo test` works on a fresh checkout; CI runs `scripts/gen-fixtures.sh`
//! first, so there the assertions really execute.

use std::path::PathBuf;

use sub_core::{CancelToken, JobService, Priority};
use sub_media::{
    Proxy, ProxyOptions, ProxyPolicy, ProxyScale, PtsIndex, proxy_size, spawn_proxy_job,
};

/// The constant-rate fixture: 125 frames at 25 fps, five seconds of bars.
const CLIP: &str = "bars_1080p_h264.mp4";

/// The variable-frame-rate fixture: frame durations genuinely vary.
const VFR_CLIP: &str = "vfr_60_30.mkv";

/// The 4K fixture, which is what an auto-generation policy triggers on.
const UHD_CLIP: &str = "bars_2160p_h264.mp4";

/// Quarter resolution keeps these transcodes to a second or two.
fn options() -> ProxyOptions {
    ProxyOptions {
        scale: ProxyScale::Quarter,
        quality: 60,
        ..ProxyOptions::default()
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
    let dir = std::env::temp_dir().join(format!("sub-proxy-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a sidecar directory");
    dir
}

#[test]
fn a_proxy_is_a_real_movie_at_the_requested_size() {
    let Some(clip) = fixture(CLIP) else { return };
    let dir = sidecar("size");
    let options = options();

    let proxy = Proxy::generate(&clip, &dir, options).expect("a generated proxy");
    let source = sub_media::probe(&clip).expect("the source probes");
    let stream = source.video.first().expect("a video stream");
    let (width, height) = proxy_size(stream.width, stream.height, options.scale);
    assert_eq!((proxy.width(), proxy.height()), (width, height));
    assert_eq!((width, height), (480, 270));

    // The written file is a movie of its own: it probes, it carries video, and
    // it is the size the proxy claims.
    let probed = sub_media::probe(&proxy.path()).expect("the proxy probes");
    let written = probed.video.first().expect("the proxy carries video");
    assert_eq!((written.width, written.height), (width, height));
    assert_eq!(
        written.codec,
        options.codec.coded_media_type(),
        "the proxy is intra-only"
    );

    // A proxy is smaller than what it stands in for, or it is pointless.
    let source_bytes = std::fs::metadata(&clip).expect("source size").len();
    let proxy_bytes = std::fs::metadata(proxy.path()).expect("proxy size").len();
    assert!(
        proxy_bytes > 0 && proxy_bytes < source_bytes,
        "{proxy_bytes}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_source_frame_has_exactly_one_proxy_frame() {
    let Some(clip) = fixture(CLIP) else { return };
    let dir = sidecar("frames");

    let proxy = Proxy::generate(&clip, &dir, options()).expect("a generated proxy");
    let source = PtsIndex::build(&clip).expect("a source index");
    assert_eq!(proxy.frame_count(), source.len());
    assert_eq!(source.len(), 125, "the fixture holds 125 frames");

    let written = PtsIndex::build(&proxy.path()).expect("a proxy index");
    assert_eq!(written.len(), source.len());
    // An intra-only proxy is all keyframes: that is the whole point of it.
    assert!(
        (0..written.len()).all(|frame| written.is_keyframe(frame) == Some(true)),
        "a proxy frame is not a keyframe"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_variable_frame_rate_source_maps_frame_for_frame() {
    let Some(clip) = fixture(VFR_CLIP) else {
        return;
    };
    let dir = sidecar("vfr");

    let source = PtsIndex::build(&clip).expect("a source index");
    let proxy = Proxy::generate(&clip, &dir, options()).expect("a generated proxy");
    assert_eq!(proxy.frame_count(), source.len());

    // The frames sit where the source's frames sit — which is what a caller
    // relies on when it shows proxy frame n for original frame n. The source
    // is deliberately not constant-rate, so this cannot be passing by
    // accident.
    let written = PtsIndex::build(&proxy.path()).expect("a proxy index");
    assert_eq!(written.len(), source.len());
    // Offsets from each file's own first frame: a container may start its
    // timeline where it likes, but the spacing of the pictures must survive.
    let source_origin = source.entries()[0].pts_ns;
    let proxy_origin = written.entries()[0].pts_ns;
    for (frame, (want, got)) in source.entries().iter().zip(written.entries()).enumerate() {
        let wanted = want.pts_ns - source_origin;
        let got = got.pts_ns - proxy_origin;
        assert!(
            (wanted - got).abs() <= sub_media::MAX_PROXY_PTS_DRIFT_NS,
            "frame {frame}: source {wanted} ns, proxy {got} ns"
        );
    }
    let durations: Vec<i64> = (0..source.len().min(8))
        .filter_map(|frame| source.duration_of(frame).map(sub_time::RationalTime::value))
        .collect();
    assert!(
        durations.windows(2).any(|pair| pair[0] != pair[1]),
        "the VFR fixture must really vary: {durations:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_finished_proxy_is_reused_rather_than_transcoded_again() {
    let Some(clip) = fixture(CLIP) else { return };
    let dir = sidecar("reuse");
    let options = options();

    let first = Proxy::generate(&clip, &dir, options).expect("a generated proxy");
    let written = std::fs::metadata(first.path()).expect("the proxy file");

    // A load alone must find it, with no source, probe or transcode involved.
    let loaded =
        Proxy::load(&dir, first.source_hash(), options).expect("the proxy reads back from disk");
    assert_eq!(loaded, first);

    let mut progress = Vec::new();
    let second = Proxy::generate_with(
        &clip,
        &dir,
        options,
        &CancelToken::new(),
        &mut |done, total| {
            progress.push((done, total));
        },
    )
    .expect("the second run");
    assert_eq!(second, first);
    assert_eq!(
        progress,
        vec![(125, 125)],
        "a reused proxy reports itself finished at once"
    );
    assert_eq!(
        std::fs::metadata(second.path())
            .expect("the proxy file")
            .modified()
            .ok(),
        written.modified().ok(),
        "the proxy was rewritten"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cancelled_generation_leaves_nothing_behind() {
    let Some(clip) = fixture(CLIP) else { return };
    let dir = sidecar("cancel");
    let cancel = CancelToken::new();
    cancel.cancel();

    let err = Proxy::generate_with(&clip, &dir, options(), &cancel, &mut |_, _| {})
        .expect_err("a cancelled generation");
    assert_eq!(err.code, sub_core::codes::CANCELLED);
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .expect("the sidecar directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_job_reports_progress_in_frames_and_hands_back_the_proxy() {
    let Some(clip) = fixture(CLIP) else { return };
    let dir = sidecar("job");
    let jobs = JobService::with_default_workers();
    let events = jobs.subscribe();

    let job = spawn_proxy_job(&jobs, &clip, &dir, options(), Priority::Background);
    let proxy = job.wait().expect("the job generates a proxy");
    assert_eq!(proxy.frame_count(), 125);
    assert!(proxy.path().is_file());

    let mut totals = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let sub_core::JobEvent::Progress { done, total, .. } = event {
            assert!(done <= total, "{done}/{total}");
            totals.push(total);
        }
    }
    assert!(
        totals.iter().all(|total| *total == 125),
        "progress is counted in source frames: {totals:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn auto_generation_triggers_on_the_4k_fixture_and_not_on_the_hd_one() {
    let (Some(uhd), Some(hd)) = (fixture(UHD_CLIP), fixture(CLIP)) else {
        return;
    };
    let policy = ProxyPolicy::default();

    let uhd_info = sub_media::probe(&uhd).expect("the 4K fixture probes");
    assert!(policy.should_generate(&uhd_info));
    let options = policy.options_for(&uhd_info).expect("options for 4K");
    let stream = uhd_info.video.first().expect("a video stream");
    let (width, height) = proxy_size(stream.width, stream.height, options.scale);
    assert_eq!((width, height), (1920, 1080));
    assert!(
        options.codec.is_available_at(width, height),
        "the policy only picks a codec this installation can write"
    );

    let hd_info = sub_media::probe(&hd).expect("the HD fixture probes");
    assert!(!policy.should_generate(&hd_info));
    assert!(policy.options_for(&hd_info).is_none());
}
