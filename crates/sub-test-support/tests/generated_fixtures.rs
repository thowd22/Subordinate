//! Checks the fixtures the generator script actually produced.
//!
//! Skips itself when no manifest is present, so `cargo test` still works on a
//! checkout where `scripts/gen-fixtures.sh` has not been run. CI runs the
//! generator before the test step, so there the assertions really execute.

use sub_test_support::{FixtureError, fixture, fixtures_dir, load_manifest, manifest_path};

#[test]
fn every_generated_fixture_resolves() {
    let manifest = match load_manifest() {
        Ok(manifest) => manifest,
        Err(FixtureError::ManifestMissing { .. }) => {
            eprintln!(
                "skipping: no fixture manifest at {}",
                manifest_path().display()
            );
            return;
        }
        Err(e) => panic!("[{}] {e}", e.code()),
    };

    assert!(
        !manifest.fixtures.is_empty(),
        "the manifest catalogue is empty"
    );
    assert!(
        manifest.generator.contains("gen-fixtures"),
        "unexpected generator {:?}",
        manifest.generator
    );

    for entry in &manifest.fixtures {
        assert!(entry.duration_ns > 0, "{} has no duration", entry.name);
        assert!(
            entry.fps_den > 0,
            "{} has a zero fps denominator",
            entry.name
        );
        if entry.generated {
            let path = fixture(&entry.name).unwrap_or_else(|e| panic!("[{}] {e}", e.code()));
            let size = std::fs::metadata(&path)
                .unwrap_or_else(|e| panic!("cannot stat {}: {e}", path.display()))
                .len();
            assert!(size > 0, "{} is empty", path.display());
        } else {
            let err = fixture(&entry.name).expect_err("a skipped fixture must not resolve");
            assert_eq!(err.code(), "fixtures.not_generated");
        }
    }

    // The catalogue must keep covering every media shape the media tasks need.
    for required in [
        "bars_1080p_h264.mp4",
        "bars_2160p_h264.mp4",
        "dropframe_2997_h264.mp4",
        "vfr_60_30.mkv",
        "longgop_720p_10min.mp4",
        "tone_48k_stereo.wav",
        "tone_48k_stereo.flac",
    ] {
        assert!(
            manifest.get(required).is_some(),
            "{required} is missing from {}",
            fixtures_dir().display()
        );
    }
    assert!(
        manifest.fixtures.iter().any(|f| f.vfr),
        "the catalogue needs a variable-frame-rate fixture"
    );
}
