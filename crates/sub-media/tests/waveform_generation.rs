//! Waveform generation against a real decoded file (TASK-53).
//!
//! These tests judge the job by what lands in the sidecar directory: one peak
//! file per zoom level, sized exactly as the manifest says, holding the shape
//! of the signal that was decoded. They then prove the two properties a
//! background job needs to be usable — that a second run decodes nothing, and
//! that a run interrupted half way finishes the rest rather than starting over.
//!
//! No committed fixture carries audio in a form these tests can assert peaks
//! against, so they synthesise their own file the way `tests/audio_decode.rs`
//! does: a `gst-launch`-shaped pipeline writing a 440 Hz sine into a WAV, which
//! is lossless, so the loudest peak is exactly the amplitude that went in. The
//! file is written once per machine into the temporary directory and reused.
//!
//! Everything skips itself when this installation cannot build that pipeline,
//! so `cargo test` still works on a bare checkout.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;
use sub_core::{CancelToken, JobEvent, JobService, Priority};
use sub_media::{Peak, Waveform, WaveformOptions, spawn_waveform_job};
use sub_model::ContentHash;

/// Sample rate the synthesised file carries.
const RATE: u32 = 48_000;

/// Frames per buffer the source emits: 4800 at 48 kHz is 100 ms.
const FRAMES_PER_BUFFER: u64 = 4_800;

/// Buffers in the file: 50 of them is exactly five seconds.
const BUFFERS: u64 = 50;

/// Audio frames the synthesised file holds, exactly.
const TOTAL_FRAMES: u64 = FRAMES_PER_BUFFER * BUFFERS;

/// Amplitude of the sine `audiotestsrc` generates, on every channel.
const SOURCE_PEAK: f32 = 0.8;

/// The elements the synthesis pipeline needs.
const REQUIRED_ELEMENTS: &[&str] = &["audiotestsrc", "audioconvert", "wavenc"];

/// Held while the file is synthesised: the tests run in parallel and all of
/// them want it.
static SYNTHESIS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Small pyramids keep these tests to a handful of files each.
fn options() -> WaveformOptions {
    WaveformOptions {
        frames_per_peak: 512,
        levels: 4,
        decimation: 4,
    }
}

/// A private sidecar directory for one test.
fn sidecar(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sub-waves-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a sidecar directory");
    dir
}

/// Whether this installation can build the synthesis pipeline.
fn encoders_available() -> bool {
    gst::init().expect("GStreamer must initialise");
    let missing: Vec<&str> = REQUIRED_ELEMENTS
        .iter()
        .copied()
        .filter(|name| gst::ElementFactory::find(name).is_none())
        .collect();
    if missing.is_empty() {
        return true;
    }
    eprintln!("skipping: this installation lacks {}", missing.join(", "));
    false
}

/// Synthesises — or reuses — the five-second stereo WAV these tests read.
/// `None` when the encoders are missing.
fn tone_file() -> Option<PathBuf> {
    if !encoders_available() {
        return None;
    }
    let _guard = SYNTHESIS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = std::env::temp_dir().join("sub-media-waveform-fixtures");
    std::fs::create_dir_all(&dir).expect("the fixture directory must be creatable");
    let path = dir.join("tone_48k_stereo.wav");
    if path.metadata().is_ok_and(|meta| meta.len() > 0) {
        return Some(path);
    }
    // Written under a temporary name and renamed, so a run that dies halfway
    // cannot leave a truncated file for the next one to decode.
    let partial = dir.join(format!("tone.{}.part", std::process::id()));
    let description = format!(
        "audiotestsrc wave=sine freq=440 volume={SOURCE_PEAK} \
         samplesperbuffer={FRAMES_PER_BUFFER} num-buffers={BUFFERS} \
         ! audio/x-raw,format=S16LE,rate={RATE},channels=2 \
         ! audioconvert ! wavenc ! filesink location={location}",
        location = partial.display().to_string().replace('\\', "/"),
    );
    run_to_eos(&description);
    std::fs::rename(&partial, &path).expect("the synthesised file must be renamable");
    Some(path)
}

/// Runs one `gst-launch`-shaped pipeline until it reaches end of stream.
fn run_to_eos(description: &str) {
    let pipeline = gst::parse::launch(description)
        .unwrap_or_else(|e| panic!("the synthesis pipeline must build: {e}"))
        .downcast::<gst::Pipeline>()
        .expect("parse produces a pipeline");
    pipeline
        .set_state(gst::State::Playing)
        .expect("the synthesis pipeline must play");
    let bus = pipeline.bus().expect("a pipeline has a bus");
    let deadline = Instant::now() + Duration::from_mins(1);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "the synthesis pipeline must reach EOS");
        if let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(
            u64::try_from(left.as_millis().min(500)).unwrap_or(500),
        )) {
            match message.view() {
                gst::MessageView::Eos(_) => break,
                gst::MessageView::Error(err) => panic!("synthesis failed: {}", err.error()),
                _ => {}
            }
        }
    }
    pipeline
        .set_state(gst::State::Null)
        .expect("the synthesis pipeline must stop");
}

/// The hash the waveform files of `path` are named after.
fn hash(path: &Path) -> ContentHash {
    ContentHash::of_file(path).expect("the source must be hashable")
}

/// The loudest excursion in a level, as a fraction of full scale.
fn loudest(peaks: &[Peak]) -> f32 {
    peaks
        .iter()
        .map(|peak| peak.max_f32().max(-peak.min_f32()))
        .fold(0.0_f32, f32::max)
}

#[test]
fn a_pyramid_of_peak_files_lands_in_the_sidecar_directory() {
    let Some(tone) = tone_file() else { return };
    let dir = sidecar("pyramid");
    let options = options();

    let waveform = Waveform::generate(&tone, &dir, options)
        .unwrap_or_else(|err| panic!("[{}] {err}", err.code));

    assert_eq!(waveform.options(), options);
    assert_eq!(waveform.sample_rate(), RATE);
    assert_eq!(waveform.channels(), 2, "the decoder folds to stereo");
    assert_eq!(waveform.frames(), TOTAL_FRAMES, "five seconds at 48 kHz");
    assert_eq!(waveform.duration().value(), 240_000);
    assert_eq!(waveform.levels().len(), options.levels as usize);

    let mut expected_peaks = TOTAL_FRAMES.div_ceil(u64::from(options.frames_per_peak));
    for level in waveform.levels() {
        assert_eq!(level.peaks, expected_peaks, "level {}", level.index);
        assert_eq!(
            level.frames_per_peak,
            options.frames_per_peak_at(level.index)
        );
        // The file is exactly as long as the manifest says it is.
        let written = std::fs::metadata(level.path(&dir))
            .expect("a level file")
            .len();
        assert_eq!(written, level.byte_len(waveform.channels()));

        let peaks = waveform.read_peaks(level.index).expect("the level's peaks");
        assert_eq!(
            peaks.len() as u64,
            level.peaks * u64::from(waveform.channels())
        );
        // A 440 Hz sine at 0.8 full scale fills every peak of every level:
        // even the coarsest bucket covers many cycles.
        assert!(
            (loudest(&peaks) - SOURCE_PEAK).abs() < 0.01,
            "level {} peaks at {}, not {SOURCE_PEAK}",
            level.index,
            loudest(&peaks)
        );
        assert!(
            peaks.iter().all(|peak| peak.min < 0 && peak.max > 0),
            "a sine swings both ways in every bucket of level {}",
            level.index
        );
        expected_peaks = expected_peaks.div_ceil(u64::from(options.decimation));
    }

    // The waveform reads back from the manifest alone.
    let loaded = Waveform::load(&dir, waveform.source_hash(), options).expect("a waveform on disk");
    assert_eq!(loaded, waveform);
    // And a level that was never asked for is an error, not a panic.
    let err = waveform
        .read_peaks(options.levels)
        .expect_err("there is no such level");
    assert_eq!(err.code.as_str(), "media.waveform_failed");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_second_run_reuses_the_levels_and_decodes_nothing() {
    let Some(tone) = tone_file() else { return };
    let dir = sidecar("reuse");
    let options = options();

    let first = Waveform::generate(&tone, &dir, options)
        .unwrap_or_else(|err| panic!("[{}] {err}", err.code));
    let base = first.level(0).expect("a base level").path(&dir);
    let modified = std::fs::metadata(&base)
        .and_then(|meta| meta.modified())
        .expect("a modification time");

    let again = Waveform::generate(&tone, &dir, options).expect("the waveform again");
    assert_eq!(again, first);
    assert_eq!(
        std::fs::metadata(&base)
            .and_then(|meta| meta.modified())
            .expect("a modification time"),
        modified,
        "a generated level must not be written again"
    );

    // Now prove that a resumed run derives the coarser levels from the base
    // level on disk rather than decoding again: replace the base peaks with
    // silence, throw the coarser levels away, and generate. Every level that
    // comes back must be silent, which only the peaks on disk could have made
    // it.
    let silent = vec![
        0_u8;
        usize::try_from(std::fs::metadata(&base).expect("a level").len())
            .expect("a small level")
    ];
    std::fs::write(&base, &silent).expect("the base level must be writable");
    for level in &first.levels()[1..] {
        std::fs::remove_file(level.path(&dir)).expect("a level file");
    }

    let resumed = Waveform::generate(&tone, &dir, options).expect("the resumed waveform");
    assert_eq!(resumed.levels(), first.levels());
    for level in resumed.levels() {
        let peaks = resumed.read_peaks(level.index).expect("the level's peaks");
        assert!(
            peaks.iter().all(|peak| *peak == Peak::SILENCE),
            "level {} was decoded again instead of being derived from the base level",
            level.index
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cancelled_run_keeps_what_it_finished_and_the_next_one_completes_it() {
    let Some(tone) = tone_file() else { return };
    let dir = sidecar("cancel");
    let options = options();

    // Cancel as soon as the base level is on disk: the job is then provably in
    // the middle of the pyramid rather than not started.
    let cancel = CancelToken::new();
    let err = Waveform::generate_with(&tone, &dir, options, &cancel, &mut |done, _| {
        if done == 1 {
            cancel.cancel();
        }
    })
    .expect_err("a cancelled run");
    assert_eq!(err.code.as_str(), "core.cancelled");
    assert!(
        Waveform::load(&dir, hash(&tone), options).is_none(),
        "a cancelled run must not leave a complete waveform behind"
    );
    let base = dir.join(Waveform::level_file_name(hash(&tone), options, 0));
    assert!(base.is_file(), "the level it finished must survive");
    let modified = std::fs::metadata(&base)
        .and_then(|meta| meta.modified())
        .expect("a modification time");

    let resumed = Waveform::generate(&tone, &dir, options).expect("the resumed waveform");
    assert_eq!(resumed.levels().len(), options.levels as usize);
    assert_eq!(
        std::fs::metadata(&base)
            .and_then(|meta| meta.modified())
            .expect("a modification time"),
        modified,
        "the level the cancelled run finished must be kept, not redone"
    );
    assert!(Waveform::load(&dir, hash(&tone), options).is_some());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_job_service_runs_a_waveform_off_the_calling_thread_and_reports_progress() {
    let Some(tone) = tone_file() else { return };
    let dir = sidecar("job");
    let options = options();

    let jobs = JobService::new(1);
    let events = jobs.subscribe();
    let job = spawn_waveform_job(&jobs, &tone, &dir, options, Priority::Background);
    let waveform = job
        .wait()
        .unwrap_or_else(|err| panic!("[{}] {err}", err.code));
    assert_eq!(waveform.levels().len(), options.levels as usize);

    let progress: Vec<(u64, u64)> = events
        .try_iter()
        .filter_map(|event| match event {
            JobEvent::Progress { done, total, .. } => Some((done, total)),
            _ => None,
        })
        .collect();
    assert_eq!(
        progress,
        (1..=u64::from(options.levels))
            .map(|done| (done, u64::from(options.levels)))
            .collect::<Vec<_>>(),
        "one progress report per finished level"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_with_no_audio_is_reported_rather_than_drawn() {
    let dir = sidecar("silent");
    let missing = dir.join("not-a-media-file.wav");
    std::fs::write(&missing, b"this is not audio").expect("a file to try");

    let err = Waveform::generate(&missing, &dir, WaveformOptions::default())
        .expect_err("nonsense bytes cannot be drawn");
    assert_eq!(err.code.domain(), "media");

    let _ = std::fs::remove_dir_all(&dir);
}
