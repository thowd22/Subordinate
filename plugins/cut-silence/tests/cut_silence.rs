//! The plugin end to end, minus the component: sound in, cuts out.
//!
//! The three modules under test each have their own unit tests; this is the
//! seam between them, which is where a plugin like this actually goes wrong. It
//! builds a `.wav` with two pauses in it, reads it the way `analyze` does,
//! measures it the way `analyze` does, and plans the cut the way `run` does —
//! all on the host triple, with no component, no host and no audio hardware.
//!
//! What it cannot cover is the Command API conversation itself. That is what
//! `subordinate-cli plugin test com.subordinate.cut-silence` is for: it runs
//! the built component against `fixture/fixture.sub` and undoes it again.

use std::collections::BTreeMap;

use subordinate_plugin_cut_silence::plan::{self, Clip, Range, Rate};
use subordinate_plugin_cut_silence::silence::{self, Options};
use subordinate_plugin_cut_silence::wav::Wav;

/// The media's sample rate, and so the analyzer's timebase.
const SAMPLE_RATE: u32 = 48_000;

/// The sequence's frame rate: 25 fps, where a second is 25 ticks.
const SEQUENCE: Rate = Rate {
    numerator: 25,
    denominator: 1,
};

/// A 16-bit mono `.wav` at 48 kHz: `pattern` is a run of seconds, each either
/// full-scale tone or digital silence.
fn dialogue(pattern: &[(u32, bool)]) -> Vec<u8> {
    let mut samples: Vec<i16> = Vec::new();
    for &(seconds, loud) in pattern {
        for frame in 0..seconds * SAMPLE_RATE {
            samples.push(if loud {
                if frame % 2 == 0 { 16_000 } else { -16_000 }
            } else {
                0
            });
        }
    }

    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut fmt = Vec::new();
    fmt.extend(1_u16.to_le_bytes());
    fmt.extend(1_u16.to_le_bytes());
    fmt.extend(SAMPLE_RATE.to_le_bytes());
    fmt.extend((SAMPLE_RATE * 2).to_le_bytes());
    fmt.extend(2_u16.to_le_bytes());
    fmt.extend(16_u16.to_le_bytes());

    let mut out = Vec::new();
    out.extend(b"RIFF");
    out.extend((4 + 8 + fmt.len() as u32 + 8 + data.len() as u32).to_le_bytes());
    out.extend(b"WAVE");
    out.extend(b"fmt ");
    out.extend((fmt.len() as u32).to_le_bytes());
    out.extend(&fmt);
    out.extend(b"data");
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(&data);
    out
}

#[test]
fn a_file_with_two_pauses_becomes_two_cuts_on_the_timeline() {
    // Ten seconds: two of speech, two of silence, two of speech, one of
    // silence, three of speech.
    let bytes = dialogue(&[(2, true), (2, false), (2, true), (1, false), (3, true)]);
    let wav = Wav::parse(&bytes).expect("the analyzer reads its own file");
    assert_eq!(wav.sample_rate, SAMPLE_RATE);
    assert_eq!(wav.frame_count(), u64::from(10 * SAMPLE_RATE));

    // The analyzer's own defaults, except that no padding is kept, so the
    // spans below are exactly where the silence is.
    let options = Options {
        padding_seconds: 0.0,
        ..Options::default()
    };
    let spans = silence::detect(wav.frames(), &options.at_rate(wav.sample_rate));
    assert_eq!(spans.len(), 2, "{spans:?}");
    assert_eq!(spans[0].start, u64::from(2 * SAMPLE_RATE));
    assert_eq!(spans[0].end, u64::from(4 * SAMPLE_RATE));
    assert_eq!(spans[1].start, u64::from(6 * SAMPLE_RATE));
    assert_eq!(spans[1].end, u64::from(7 * SAMPLE_RATE));

    // One clip playing the whole file, at the head of a 25 fps sequence.
    let media = "media-1".to_owned();
    let clips = [Clip {
        id: "clip-1".to_owned(),
        track: "track-1".to_owned(),
        media: media.clone(),
        timeline: Range::new(0, 250, SEQUENCE),
        source: Range::new(
            0,
            i64::from(10 * SAMPLE_RATE),
            Rate::per_second(SAMPLE_RATE),
        ),
    }];
    let mut silence = BTreeMap::new();
    silence.insert(
        media,
        spans
            .iter()
            .map(|span| {
                Range::new(
                    span.start as i64,
                    span.len() as i64,
                    Rate::per_second(SAMPLE_RATE),
                )
            })
            .collect::<Vec<_>>(),
    );

    // Two seconds is 50 frames and one second is 25, and the cuts come back
    // last first so that applying them in order is safe.
    let cuts = plan::plan(&clips, &silence, SEQUENCE);
    assert_eq!(cuts.len(), 2);
    assert_eq!((cuts[0].start, cuts[0].end), (150, 175));
    assert_eq!((cuts[1].start, cuts[1].end), (50, 100));
    let removed: i64 = cuts.iter().map(plan::Cut::len).sum();
    assert_eq!(removed, 75, "three seconds of silence at 25 fps");
}

#[test]
fn a_file_with_no_silence_in_it_plans_no_cuts() {
    let bytes = dialogue(&[(5, true)]);
    let wav = Wav::parse(&bytes).expect("a wav");
    let spans = silence::detect(wav.frames(), &Options::default().at_rate(wav.sample_rate));
    assert!(spans.is_empty(), "{spans:?}");

    let clips = [Clip {
        id: "clip-1".to_owned(),
        track: "track-1".to_owned(),
        media: "media-1".to_owned(),
        timeline: Range::new(0, 125, SEQUENCE),
        source: Range::new(0, i64::from(5 * SAMPLE_RATE), Rate::per_second(SAMPLE_RATE)),
    }];
    assert!(plan::plan(&clips, &BTreeMap::new(), SEQUENCE).is_empty());
}

#[test]
fn raising_the_threshold_finds_the_quiet_passage_a_lower_one_missed() {
    // A passage at about -54 dBFS: under a -50 dB threshold it is silence,
    // over a -60 dB one it is not.
    let mut samples: Vec<i16> = vec![16_000; (2 * SAMPLE_RATE) as usize];
    samples.extend(std::iter::repeat_n(64_i16, (2 * SAMPLE_RATE) as usize));
    samples.extend(std::iter::repeat_n(16_000_i16, (2 * SAMPLE_RATE) as usize));

    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut fmt = Vec::new();
    fmt.extend(1_u16.to_le_bytes());
    fmt.extend(1_u16.to_le_bytes());
    fmt.extend(SAMPLE_RATE.to_le_bytes());
    fmt.extend((SAMPLE_RATE * 2).to_le_bytes());
    fmt.extend(2_u16.to_le_bytes());
    fmt.extend(16_u16.to_le_bytes());
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((4 + 8 + fmt.len() as u32 + 8 + data.len() as u32).to_le_bytes());
    bytes.extend(b"WAVE");
    bytes.extend(b"fmt ");
    bytes.extend((fmt.len() as u32).to_le_bytes());
    bytes.extend(&fmt);
    bytes.extend(b"data");
    bytes.extend((data.len() as u32).to_le_bytes());
    bytes.extend(&data);

    let wav = Wav::parse(&bytes).expect("a wav");
    let quiet = Options {
        threshold_db: -50.0,
        padding_seconds: 0.0,
        ..Options::default()
    };
    let strict = Options {
        threshold_db: -60.0,
        padding_seconds: 0.0,
        ..Options::default()
    };
    assert_eq!(
        silence::detect(wav.frames(), &quiet.at_rate(wav.sample_rate)).len(),
        1
    );
    assert!(silence::detect(wav.frames(), &strict.at_rate(wav.sample_rate)).is_empty());
}
