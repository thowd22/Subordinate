//! Importing a real file through the queue: hash, probe and thumbnails, all as
//! jobs (TASK-35).
//!
//! This is the half of import that headless egui cannot reach, because it
//! needs actual media. The generated fixture is copied into a temporary
//! project folder, submitted to an [`ImportQueue`], and judged by what comes
//! back: an item carrying the content hash and the probe result the project
//! file will store, and a strip of JPEGs in the sidecar folder that the media
//! bin can draw without decoding anything itself.
//!
//! The test skips itself when the fixtures have not been generated, so
//! `cargo test` works on a fresh checkout; CI runs `scripts/gen-fixtures.sh`
//! first, so there it really executes.

use std::path::PathBuf;

use sub_core::JobService;
use sub_edit::History;
use sub_edit::commands::ImportMedia;
use sub_media::{ThumbnailOptions, ThumbnailStrip, Waveform, WaveformOptions};
use sub_model::Project;
use sub_time::Rational;
use sub_ui::media_import::{ImportOptions, ImportOutcome, ImportQueue};

/// The constant-rate fixture: 125 frames at 25 fps, five seconds of bars.
/// Video only, so it gets a thumbnail strip and no waveform.
const CLIP: &str = "bars_1080p_h264.mp4";

/// The audio fixture: five seconds of tone. Sound only, so it gets a waveform
/// and no thumbnail strip.
const TONE: &str = "tone_48k_stereo.wav";

/// Small strips keep this test to a few seeks.
fn options() -> ImportOptions {
    ImportOptions {
        thumbnails: ThumbnailOptions {
            count: 3,
            max_width: 160,
            quality: 70,
        },
        ..ImportOptions::default()
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

/// A private project folder holding a copy of each fixture, and its sidecar.
fn project_folder(fixtures: &[(&str, PathBuf)]) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("sub-ui-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("footage")).expect("a project folder");
    let sidecar = dir.join("cut.sub.d");
    std::fs::create_dir_all(&sidecar).expect("a sidecar folder");
    for (name, source) in fixtures {
        std::fs::copy(source, dir.join("footage").join(name))
            .expect("the fixture copies into the project folder");
    }
    (dir, sidecar)
}

#[test]
fn importing_a_file_hashes_probes_and_thumbnails_it_off_the_ui_thread() {
    let (Some(clip), Some(tone)) = (fixture(CLIP), fixture(TONE)) else {
        return;
    };
    let (dir, sidecar) = project_folder(&[(CLIP, clip), (TONE, tone)]);
    let jobs = JobService::new(2);
    let mut queue = ImportQueue::new(&dir, &sidecar).with_options(options());

    // Both files in one gesture, which is one batch and therefore one entry in
    // the undo stack once the host applies it.
    queue.submit(
        &jobs,
        &[
            dir.join("footage").join(CLIP),
            dir.join("footage").join(TONE),
        ],
        None,
    );
    assert_eq!(queue.importing(), 2, "each import runs as its own job");

    // The UI thread would poll once a frame; here one wait stands in for that.
    let deadline = std::time::Instant::now() + std::time::Duration::from_mins(1);
    let mut finished = Vec::new();
    while finished.is_empty() && std::time::Instant::now() < deadline {
        finished = queue.poll(&jobs);
    }
    assert_eq!(finished.len(), 1, "one gesture, one batch");
    let mut batch = finished.remove(0);
    assert_eq!(
        batch.outcomes.len(),
        2,
        "a batch comes back whole, never half"
    );
    assert_eq!(batch.bin, None, "an unfiled gesture lands in the root bin");

    // The outcomes come back in the order the files were asked for, not the
    // order the jobs happened to finish.
    let ImportOutcome::Ready { item: sound, .. } = batch.outcomes.remove(1) else {
        panic!("the audio import failed");
    };
    assert_eq!(sound.path.as_str(), "footage/tone_48k_stereo.wav");
    let sound_hash = sound.hash.expect("the audio bytes were hashed");
    let sound_info = sound.info.as_ref().expect("the audio was probed");
    assert!(sound_info.video.is_empty(), "the tone has no picture");
    assert_eq!(sound_info.audio[0].sample_rate, 48_000);

    let ImportOutcome::Ready { item, bin } = batch.outcomes.remove(0) else {
        panic!("the import failed");
    };
    assert_eq!(bin, None, "an unfiled import lands in the root bin");
    assert_eq!(item.path.as_str(), "footage/bars_1080p_h264.mp4");
    assert_eq!(item.name, "bars_1080p_h264.mp4");
    assert!(item.hash.is_some(), "the bytes were hashed");
    assert!(!item.offline);

    let info = item.info.as_ref().expect("the file was probed");
    let video = info.video.first().expect("the fixture has video");
    assert_eq!((video.width, video.height), (1920, 1080));
    assert_eq!(video.frame_rate, Rational::FPS_25);
    let duration = info.duration.expect("the fixture declares a duration");
    assert_eq!(
        duration.rescaled_to(Rational::FPS_25).value(),
        125,
        "five seconds at 25 fps, exactly"
    );

    // The item is what the host applies as a command, so the import is
    // undoable like every other edit.
    let mut project = Project::new("Doc cut");
    let mut history = History::new();
    let id = item.id;
    let hash = item.hash;
    history
        .apply(&mut project, ImportMedia::new(*item))
        .expect("the item imports");
    assert_eq!(project.bin_of(id), Some(project.root_bin.id));
    history.undo(&mut project).expect("the import undoes");
    assert!(project.media.is_empty());

    // The strip and the waveform were queued off the back of the import and
    // land in the sidecar folder, both named after the content hash.
    jobs.wait_idle();
    assert_eq!(queue.poll(&jobs).len(), 0, "nothing else was imported");
    assert!(queue.is_idle(), "every job finished");

    let hash = hash.expect("hashed");
    // A follow-up job only runs for a stream the file actually has: the bars
    // get a strip and no waveform, the tone a waveform and no strip.
    assert!(
        Waveform::load(&sidecar, hash, WaveformOptions::default()).is_none(),
        "a silent file queues no waveform"
    );
    let waveform = Waveform::load(&sidecar, sound_hash, WaveformOptions::default())
        .expect("the audio import queued a waveform of its own");
    assert!(
        !waveform.levels().is_empty(),
        "the tone summarises to peaks the timeline can draw"
    );
    assert!(
        ThumbnailStrip::load(&sidecar, sound_hash, options().thumbnails).is_none(),
        "a file with no picture queues no strip"
    );

    let strip = ThumbnailStrip::load(&sidecar, hash, options().thumbnails)
        .expect("the strip was generated");
    assert_eq!(strip.frames().len(), 3);
    for frame in strip.frames() {
        let path = frame.path(&sidecar);
        let bytes = std::fs::read(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        assert!(
            bytes.starts_with(&[0xFF, 0xD8]) && bytes.ends_with(&[0xFF, 0xD9]),
            "{} is not a complete JPEG",
            path.display()
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
