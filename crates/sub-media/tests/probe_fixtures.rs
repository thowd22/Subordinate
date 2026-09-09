//! Probes every generated fixture and checks the report against the manifest.
//!
//! Skips itself when the fixtures have not been generated, so `cargo test`
//! still works on a fresh checkout; CI runs `scripts/gen-fixtures.sh` first, so
//! there the assertions really execute.

use std::path::PathBuf;
use std::time::Duration;

use sub_media::{FrameTiming, ProbeOptions, probe, probe_with};
use sub_test_support::{Fixture, FixtureError, FixtureKind, Manifest, fixture, load_manifest};
use sub_time::Rational;

/// Turns on the probe's own debug logging once per test binary, so a failure
/// here reports why a scan gave up instead of only that it did.
fn log_scan_decisions() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = sub_core::logging::init("sub_media=debug");
    });
}

/// Loads the manifest, or `None` when the fixtures were never generated.
fn manifest() -> Option<Manifest> {
    log_scan_decisions();
    match load_manifest() {
        Ok(manifest) => Some(manifest),
        Err(FixtureError::ManifestMissing { .. }) => {
            eprintln!("skipping: no fixture manifest; run scripts/gen-fixtures.sh");
            None
        }
        Err(e) => panic!("[{}] {e}", e.code()),
    }
}

/// Every fixture the generator actually wrote, with its path.
fn generated(manifest: &Manifest) -> Vec<(Fixture, PathBuf)> {
    manifest
        .generated()
        .map(|entry| {
            let path = fixture(&entry.name).unwrap_or_else(|e| panic!("[{}] {e}", e.code()));
            (entry.clone(), path)
        })
        .collect()
}

/// Difference between the probed and expected duration, in nanoseconds.
fn duration_gap(probed_ns: i64, expected_ns: u64) -> i64 {
    let expected = i64::try_from(expected_ns).expect("fixture durations fit in i64");
    (probed_ns - expected).abs()
}

#[test]
fn every_fixture_probes_into_the_shape_the_manifest_promises() {
    let Some(manifest) = manifest() else { return };
    for (entry, path) in generated(&manifest) {
        let info = probe(&path).unwrap_or_else(|e| panic!("probing {}: {e}", entry.name));

        assert!(
            !info.container.is_empty() && info.container != "unknown",
            "{}: no container media type",
            entry.name
        );
        assert!(
            info.seekable,
            "{}: a local file must be seekable",
            entry.name
        );

        let duration = info
            .duration
            .unwrap_or_else(|| panic!("{}: no duration", entry.name));
        assert_eq!(
            duration.rate(),
            sub_media::probe::NANOSECONDS,
            "{}: durations are reported in nanoseconds",
            entry.name
        );
        // Containers round the last frame's end to their own timescale, so a
        // millisecond of slack is expected; a wrong duration is never that
        // close. A lossy encoder also brackets the signal with priming and
        // padding frames, which no container trims away here, so those files
        // get a tenth of a second instead: still far tighter than a wrong
        // duration, which would be out by seconds.
        let tolerance_ns = if entry.lossy { 100_000_000 } else { 1_000_000 };
        assert!(
            duration_gap(duration.value(), entry.duration_ns) < tolerance_ns,
            "{}: duration {} ns, manifest says {} ns",
            entry.name,
            duration.value(),
            entry.duration_ns
        );

        match entry.kind {
            FixtureKind::Video => check_video(&entry, &info),
            FixtureKind::Audio => {
                assert!(!info.has_video(), "{}: unexpected video stream", entry.name);
                assert!(
                    !info.is_variable_frame_rate(),
                    "{}: audio is not VFR",
                    entry.name
                );
            }
        }

        if entry.name.starts_with("tone_") {
            let audio = info
                .audio
                .first()
                .unwrap_or_else(|| panic!("{}: no audio stream", entry.name));
            assert_eq!(audio.channels, 2, "{}: channel count", entry.name);
            assert_eq!(audio.sample_rate, 48_000, "{}: sample rate", entry.name);
            assert!(
                !audio.codec.is_empty() && !audio.codec_description.is_empty(),
                "{}: audio codec not reported",
                entry.name
            );
        }
    }
}

/// Checks the video half of a fixture's report.
fn check_video(entry: &Fixture, info: &sub_media::MediaInfo) {
    let video = info
        .video
        .first()
        .unwrap_or_else(|| panic!("{}: no video stream", entry.name));

    assert_eq!(video.width, entry.width, "{}: width", entry.name);
    assert_eq!(video.height, entry.height, "{}: height", entry.name);
    assert_eq!(
        video.codec, "video/x-h264",
        "{}: every video fixture is H.264",
        entry.name
    );
    assert!(
        video.codec_description.contains("264"),
        "{}: codec description {:?}",
        entry.name,
        video.codec_description
    );
    assert_eq!(
        video.frame_rate,
        Rational::new(entry.fps_num, entry.fps_den),
        "{}: frame rate",
        entry.name
    );
    assert_eq!(
        video.sample_aspect,
        Rational::ONE,
        "{}: square pixels",
        entry.name
    );
    assert_eq!(
        video.rotation,
        sub_media::Rotation::None,
        "{}: no rotation tag was written",
        entry.name
    );
    assert!(!video.mirrored, "{}: not mirrored", entry.name);
    assert!(!video.interlaced, "{}: progressive", entry.name);
    assert_eq!(
        video.color,
        sub_model::sequence::ColorTags::REC709,
        "{}: colour tags",
        entry.name
    );

    let expected = if entry.vfr {
        FrameTiming::Variable
    } else {
        FrameTiming::Constant
    };
    assert_eq!(
        video.timing, expected,
        "{}: frame timing (manifest says vfr={})",
        entry.name, entry.vfr
    );
    assert_eq!(
        info.is_variable_frame_rate(),
        entry.vfr,
        "{}: VFR flag",
        entry.name
    );
}

#[test]
fn the_variable_frame_rate_fixture_is_the_only_variable_one() {
    let Some(manifest) = manifest() else { return };
    let mut variable = Vec::new();
    for (entry, path) in generated(&manifest) {
        if entry.kind != FixtureKind::Video {
            continue;
        }
        let info = probe(&path).unwrap_or_else(|e| panic!("probing {}: {e}", entry.name));
        if info.is_variable_frame_rate() {
            variable.push(entry.name.clone());
        }
    }
    assert_eq!(variable, vec!["vfr_60_30.mkv".to_owned()]);
}

#[test]
fn skipping_the_scan_leaves_timing_unknown() {
    let Some(_) = manifest() else { return };
    let Some(path) = sub_test_support::try_fixture("bars_1080p_h264.mp4") else {
        return;
    };
    let options = ProbeOptions {
        scan_frame_timing: false,
        ..ProbeOptions::default()
    };
    let info = probe_with(&path, options).expect("probe without the timing scan");
    assert_eq!(info.video[0].timing, FrameTiming::Unknown);
    // Everything the discoverer itself reports is still there.
    assert_eq!(info.video[0].width, 1920);
    assert!(info.duration.is_some());
}

#[test]
fn a_missing_file_is_reported_as_unreadable() {
    let err = probe(&PathBuf::from("/definitely/not/here.mp4")).expect_err("must fail");
    assert_eq!(err.code.as_str(), "media.file_unreadable");
    assert!(
        err.details.contains_key("path"),
        "the path is in the details"
    );
}

#[test]
fn a_directory_is_not_a_media_file() {
    let err = probe(&std::env::temp_dir()).expect_err("a directory is not media");
    assert_eq!(err.code.as_str(), "media.file_unreadable");
}

#[test]
fn a_corrupt_file_is_reported_with_a_media_code() {
    let dir = std::env::temp_dir().join(format!("sub-media-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let path = dir.join("corrupt.mp4");
    // An MP4 header followed by noise: typefinding recognises it, demuxing
    // cannot make sense of it.
    let mut bytes = b"\x00\x00\x00\x18ftypisom\x00\x00\x02\x00isomiso2".to_vec();
    bytes.extend(std::iter::repeat_n(0xA5_u8, 4096));
    std::fs::write(&path, &bytes).expect("write corrupt fixture");

    let err = probe(&path).expect_err("a corrupt file must not probe");
    assert!(
        matches!(
            err.code.as_str(),
            "media.probe_failed" | "media.unsupported"
        ),
        "unexpected code {}",
        err.code
    );
    assert_eq!(err.code.domain(), "media");

    std::fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn an_impossible_budget_times_out_rather_than_hanging() {
    let Some(path) = sub_test_support::try_fixture("bars_2160p_h264.mp4") else {
        eprintln!("skipping: fixtures not generated");
        return;
    };
    let options = ProbeOptions {
        timeout: Duration::from_nanos(1),
        scan_frame_timing: true,
    };
    match probe_with(&path, options) {
        // A one-nanosecond budget normally trips the discoverer's timeout, but
        // a warm page cache can beat it; either way the probe returns rather
        // than hanging, and a failure is always a media-domain error.
        Ok(info) => assert_eq!(info.video[0].timing, FrameTiming::Unknown),
        Err(err) => assert_eq!(err.code.domain(), "media"),
    }
}
