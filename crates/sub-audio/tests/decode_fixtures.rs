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
        assert_eq!(
            duration.value(),
            expected_frames,
            "{}: duration in frames",
            entry.name
        );
        assert_eq!(
            duration.rate(),
            Rational::new(48_000, 1).unwrap(),
            "{}: durations are counted in audio frames",
            entry.name
        );

        let pcm = decode_file(&path)
            .unwrap_or_else(|e| panic!("[{}] decoding {}: {e}", e.code, entry.name));
        assert_eq!(
            i64::try_from(pcm.frames()).expect("fits"),
            expected_frames,
            "{}: decoded frame count",
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

#[test]
fn a_lossy_fixture_decodes_when_the_generator_wrote_one() {
    // MP3, AAC and Ogg Vorbis fixtures are only produced where the encoders
    // are installed; where they are, they must decode to the same shape as the
    // lossless ones (lossy codecs are never sample-identical, so only the
    // shape and the signal level are checked).
    let mut seen = 0_usize;
    for name in [
        "tone_48k_stereo.mp3",
        "tone_48k_stereo.m4a",
        "tone_48k_stereo.aac",
        "tone_48k_stereo.ogg",
    ] {
        let Some(pcm) = decode_fixture(name) else {
            continue;
        };
        assert_eq!(pcm.channels, 2, "{name}: channel count");
        assert_eq!(pcm.sample_rate, 48_000, "{name}: sample rate");
        assert!(pcm.frames() > 0, "{name}: decoded no frames");
        assert!(
            pcm.samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0),
            "{name}: samples must be finite and normalised"
        );
        seen += 1;
    }
    if seen == 0 {
        eprintln!("skipping: no lossy audio fixtures in this fixture set");
    }
}
