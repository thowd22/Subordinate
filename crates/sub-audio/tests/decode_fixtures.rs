//! Decodes the generated audio fixtures with symphonia and checks them
//! against the manifest and against each other.
//!
//! Skips itself when the fixtures have not been generated, so `cargo test`
//! still works on a fresh checkout; CI runs `scripts/gen-fixtures.sh` first, so
//! there the assertions really execute.

use sub_audio::{FileDecoder, Pcm, decode_file, probe_audio};
use sub_test_support::{FixtureError, FixtureKind, Manifest, load_manifest};
use sub_time::{Rational, RationalTime};

/// Loads the manifest, or `None` when the fixtures were never generated.
fn manifest() -> Option<Manifest> {
    match load_manifest() {
        Ok(manifest) => Some(manifest),
        Err(FixtureError::ManifestMissing { .. }) => {
            eprintln!("skipping: no fixture manifest; run scripts/gen-fixtures.sh");
            None
        }
        Err(e) => panic!("[{}] {e}", e.code()),
    }
}

/// Decodes one fixture by name, or `None` when it was not generated.
fn decode_fixture(name: &str) -> Option<Pcm> {
    let path = sub_test_support::try_fixture(name)?;
    Some(decode_file(&path).unwrap_or_else(|e| panic!("[{}] decoding {name}: {e}", e.code)))
}

#[test]
fn every_audio_only_fixture_decodes_with_the_shape_the_manifest_promises() {
    let Some(manifest) = manifest() else { return };
    let mut checked = 0_usize;
    for entry in manifest.generated() {
        if entry.kind != FixtureKind::Audio {
            continue;
        }
        let path = sub_test_support::fixture(&entry.name).expect("a generated fixture exists");
        let info = probe_audio(&path)
            .unwrap_or_else(|e| panic!("[{}] probing {}: {e}", e.code, entry.name));

        // Every audio fixture the generator writes is 48 kHz stereo.
        assert_eq!(info.channels, 2, "{}: channel count", entry.name);
        assert_eq!(info.sample_rate, 48_000, "{}: sample rate", entry.name);

        let duration = info
            .duration
            .unwrap_or_else(|| panic!("{}: no duration", entry.name));
        let expected_frames =
            i64::try_from(entry.duration_ns).expect("fits") * 48_000 / 1_000_000_000_i64;
        // A lossless file holds exactly the frames that went in. A lossy
        // encoder brackets them with priming and padding, so the file is a
        // little longer than the authored length; a tenth of a second of slack
        // covers that and still catches a wrong duration, which is out by
        // seconds.
        let tolerance = if entry.lossy { 4_800 } else { 0 };
        assert!(
            (duration.value() - expected_frames).abs() <= tolerance,
            "{}: duration {} frames, manifest says {expected_frames}",
            entry.name,
            duration.value()
        );
        assert_eq!(
            duration.rate(),
            Rational::new(48_000, 1).unwrap(),
            "{}: durations are counted in audio frames",
            entry.name
        );

        let pcm = decode_file(&path)
            .unwrap_or_else(|e| panic!("[{}] decoding {}: {e}", e.code, entry.name));
        let decoded = i64::try_from(pcm.frames()).expect("fits");
        assert!(
            (decoded - expected_frames).abs() <= tolerance,
            "{}: decoded {decoded} frames, manifest says {expected_frames}",
            entry.name
        );
        assert!(
            pcm.samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0),
            "{}: samples must be finite and normalised",
            entry.name
        );
        assert!(
            pcm.samples.iter().any(|s| *s != 0.0),
            "{}: a tone fixture is not silence",
            entry.name
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "the fixture set must contain at least one audio-only file"
    );
}

#[test]
fn the_wav_and_flac_fixtures_decode_to_bit_identical_samples() {
    let (Some(wav), Some(flac)) = (
        decode_fixture("tone_48k_stereo.wav"),
        decode_fixture("tone_48k_stereo.flac"),
    ) else {
        eprintln!("skipping: audio fixtures not generated");
        return;
    };

    assert_eq!(wav.sample_rate, flac.sample_rate);
    assert_eq!(wav.channels, flac.channels);
    assert_eq!(wav.frames(), flac.frames(), "same number of frames");
    // FLAC is lossless, so the two decode paths must agree on every bit of
    // every sample, not merely to within a tolerance.
    let mismatch = wav
        .samples
        .iter()
        .zip(&flac.samples)
        .position(|(a, b)| a.to_bits() != b.to_bits());
    assert_eq!(
        mismatch, None,
        "WAV and FLAC differ at interleaved sample {mismatch:?}"
    );
}

#[test]
fn seeking_a_lossless_fixture_lands_on_the_exact_frame() {
    let Some(path) = sub_test_support::try_fixture("tone_48k_stereo.flac") else {
        eprintln!("skipping: audio fixtures not generated");
        return;
    };
    let whole = decode_fixture("tone_48k_stereo.flac").expect("the fixture decodes");
    let rate = Rational::new(whole.sample_rate, 1).unwrap();
    let channels = usize::from(whole.channels);

    let mut decoder = FileDecoder::open(&path).expect("open the flac fixture");
    for frame in [0_i64, 1, 4_001, 96_000, 239_999] {
        let target = RationalTime::new(frame, rate);
        assert_eq!(decoder.seek(target).expect("seek"), target);
        let block = decoder
            .next_block()
            .expect("decode after the seek")
            .expect("there is audio at this position");
        assert_eq!(block.start, target, "block start after seeking to {frame}");
        let offset = usize::try_from(frame).expect("fits") * channels;
        let expected = &whole.samples[offset..offset + block.samples.len()];
        let mismatch = block
            .samples
            .iter()
            .zip(expected)
            .position(|(a, b)| a.to_bits() != b.to_bits());
        assert_eq!(
            mismatch, None,
            "samples after seeking to frame {frame} differ at {mismatch:?}"
        );
    }
}

/// Root mean square of interleaved samples, as a measure of signal level.
///
/// Only used to compare what a lossy decode carries against the lossless
/// reference; no timing value in these tests is ever a float.
fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    let count = f64::from(u32::try_from(samples.len()).expect("a fixture fits in u32 samples"));
    (sum / count).sqrt()
}

#[test]
fn every_lossy_fixture_decodes_to_the_same_tone_as_the_lossless_reference() {
    // MP3, AAC and Ogg Vorbis fixtures are only produced where the encoders
    // are installed, so the manifest, not a hard-coded list, says which ones
    // to check. A lossy codec is never sample-identical, so the decode is
    // compared against the WAV reference by signal level rather than by bits.
    let Some(manifest) = manifest() else { return };
    let Some(reference) = decode_fixture("tone_48k_stereo.wav") else {
        eprintln!("skipping: audio fixtures not generated");
        return;
    };
    let reference_rms = rms(&reference.samples);

    let mut seen = 0_usize;
    for entry in manifest.generated().filter(|f| f.lossy) {
        let name = entry.name.as_str();
        let path = sub_test_support::fixture(name).expect("a generated fixture exists");
        let pcm =
            decode_file(&path).unwrap_or_else(|e| panic!("[{}] decoding {name}: {e}", e.code));

        assert_eq!(pcm.channels, 2, "{name}: channel count");
        assert_eq!(pcm.sample_rate, 48_000, "{name}: sample rate");
        assert!(
            pcm.samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0),
            "{name}: samples must be finite and normalised"
        );
        let level = rms(&pcm.samples);
        assert!(
            (level - reference_rms).abs() < reference_rms / 10.0,
            "{name}: decoded level {level} is not within 10% of the WAV's {reference_rms}"
        );
        seen += 1;
    }
    if seen == 0 {
        eprintln!("skipping: no lossy audio fixtures in this fixture set");
    }
}

#[test]
fn seeking_a_lossy_fixture_lands_on_the_requested_frame() {
    // A lossy decoder cannot be asked for the same samples a continuous decode
    // produced, because its filter bank carries state across frames. What it
    // must do is resume exactly at the requested frame and keep producing the
    // tone, which is what a timeline playhead depends on.
    let Some(manifest) = manifest() else { return };
    let mut seen = 0_usize;
    for entry in manifest.generated().filter(|f| f.lossy) {
        let name = entry.name.as_str();
        let path = sub_test_support::fixture(name).expect("a generated fixture exists");
        let mut decoder =
            FileDecoder::open(&path).unwrap_or_else(|e| panic!("[{}] opening {name}: {e}", e.code));
        let rate = Rational::new(decoder.info().sample_rate, 1).expect("a positive sample rate");
        let manifest_frames = i64::try_from(
            entry
                .duration_ns
                .saturating_mul(u64::from(decoder.info().sample_rate))
                / 1_000_000_000,
        )
        .ok();
        let duration = decoder
            .info()
            .duration
            .map(|value| value.value())
            .or(manifest_frames)
            .zip(manifest_frames)
            .map(|(reported, manifest)| reported.min(manifest));

        for frame in [0_i64, 4_001, 96_000, 200_000] {
            if duration.is_some_and(|end| frame >= end) {
                continue;
            }
            let target = RationalTime::new(frame, rate);
            let landed = decoder
                .seek(target)
                .unwrap_or_else(|e| panic!("[{}] seeking {name} to {frame}: {e}", e.code));
            assert_eq!(landed, target, "{name}: seek to frame {frame}");
            assert_eq!(decoder.position(), target, "{name}: position after seeking");
            let block = decoder
                .next_block()
                .unwrap_or_else(|e| panic!("[{}] decoding {name} after a seek: {e}", e.code))
                .unwrap_or_else(|| panic!("{name}: no audio at frame {frame}"));
            assert_eq!(block.start, target, "{name}: block start after the seek");
            assert!(
                block.samples.iter().any(|s| *s != 0.0),
                "{name}: silence after seeking to frame {frame}"
            );
        }
        seen += 1;
    }
    if seen == 0 {
        eprintln!("skipping: no lossy audio fixtures in this fixture set");
    }
}
