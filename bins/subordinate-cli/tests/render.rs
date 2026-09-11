//! `subordinate-cli render` writes a real file from a real project (TASK-63).
//!
//! The project is assembled here out of the generated fixtures — a colour-bar
//! clip on V1 and a tone on A1 — written to a scratch directory and rendered
//! by the binary itself, so what is exercised is the command a person and CI
//! actually run: arguments in, progress on stderr, a JSON report on stdout, a
//! file on disk.
//!
//! The test skips itself, saying why, when the fixtures have not been
//! generated, when this machine enumerates no wgpu adapter to composite on, or
//! when no usable H.264 encoder is installed: all three are environment
//! problems rather than defects in the command, and CI has all three.

use std::path::{Path, PathBuf};
use std::process::Command;

use sub_model::{
    Clip, ColorTags, MediaItem, MediaPath, Project, Resolution, Sequence, SequenceSettings, Track,
    TrackKind, json,
};
use sub_time::{Rational, RationalTime, TimeRange};

/// The sequence timebase, which is the rate of both video fixtures below.
const FPS: Rational = Rational::FPS_25;
/// How long the clips are, in frames: two seconds of the five each fixture
/// holds, so a range can be trimmed out of the middle.
const CLIP_FRAMES: i64 = 50;
/// The canvas: small enough to encode in a moment on a software adapter, and
/// a different shape from the 16:9 source so the letterbox fit is exercised.
const CANVAS: (u32, u32) = (320, 240);

/// The fixture carrying the picture.
const VIDEO_FIXTURE: &str = "bars_1080p_h264.mp4";
/// The fixture carrying the sound.
const AUDIO_FIXTURE: &str = "tone_48k_stereo.wav";

/// Runs the binary and hands back its status and streams.
fn run(args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_subordinate-cli"))
        .args(args)
        .env("SUBORDINATE_LOG", "warn")
        .env_remove("RUST_LOG")
        .output()
        .expect("subordinate-cli runs");
    (
        output.status.success(),
        String::from_utf8(output.stdout).expect("utf-8 stdout"),
        String::from_utf8(output.stderr).expect("utf-8 stderr"),
    )
}

/// A scratch directory of its own for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("subordinate-cli-render-{name}"));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(dir.join("media")).expect("a scratch directory");
    dir
}

/// Copies a generated fixture into the project's own `media` folder, because a
/// project file may only name paths relative to itself.
///
/// `None` means the fixtures have not been generated on this machine.
fn media(dir: &Path, name: &str) -> Option<MediaPath> {
    let source = sub_test_support::try_fixture(name)?;
    let relative = format!("media/{name}");
    std::fs::copy(&source, dir.join(&relative)).expect("the fixture copies into the project");
    Some(MediaPath::new(&relative).expect("a project-relative media path"))
}

/// A project holding one video clip, and one audio clip when `with_audio`.
fn project(dir: &Path, with_audio: bool) -> Option<PathBuf> {
    let settings = SequenceSettings::new(
        Resolution::new(CANVAS.0, CANVAS.1).expect("a valid canvas"),
        FPS,
        48_000,
        ColorTags::REC709,
    )
    .expect("valid sequence settings");
    let mut project = Project::new("render test");
    let mut sequence = Sequence::new("Main", settings);

    let picture = MediaItem::new(media(dir, VIDEO_FIXTURE)?);
    let mut video = Track::new("V1", TrackKind::Video);
    video.items.push(
        Clip::new(
            "bars",
            picture.id,
            TimeRange::new(RationalTime::zero(FPS), RationalTime::new(CLIP_FRAMES, FPS))
                .expect("a valid source range"),
        )
        .into(),
    );
    sequence.tracks.push(video);
    project.media.push(picture);

    if with_audio {
        let tone = MediaItem::new(media(dir, AUDIO_FIXTURE)?);
        let mut audio = Track::new("A1", TrackKind::Audio);
        audio.items.push(
            Clip::new(
                "tone",
                tone.id,
                TimeRange::new(RationalTime::zero(FPS), RationalTime::new(CLIP_FRAMES, FPS))
                    .expect("a valid source range"),
            )
            .into(),
        );
        sequence.tracks.push(audio);
        project.media.push(tone);
    }

    project.sequences.push(sequence);
    let path = dir.join("render.sub");
    std::fs::write(
        &path,
        json::to_json(&project).expect("the project serialises"),
    )
    .expect("the project file writes");
    Some(path)
}

/// True when this machine can composite: a render needs a wgpu adapter.
fn has_adapter() -> bool {
    match sub_render::RenderContext::headless() {
        Ok(_) => true,
        Err(sub_render::RenderError::NoAdapter { backends }) => {
            eprintln!("skipping: no wgpu adapter for backends [{backends}]");
            false
        }
        Err(error) => panic!("[{}] {error}", error.code()),
    }
}

/// True when this machine has the software H.264 encoder the tests pin.
fn has_x264() -> bool {
    if sub_media::init().is_err() {
        eprintln!("skipping: GStreamer could not be initialised");
        return false;
    }
    if sub_export::element_is_usable("x264enc") {
        return true;
    }
    eprintln!("skipping: x264enc is not installed on this machine");
    false
}

/// The scratch project, or `None` when this machine cannot run the test.
fn ready(name: &str, with_audio: bool) -> Option<(PathBuf, PathBuf)> {
    if !has_adapter() || !has_x264() {
        return None;
    }
    let dir = scratch(name);
    let Some(project) = project(&dir, with_audio) else {
        eprintln!("skipping: no generated fixtures; run scripts/gen-fixtures.sh");
        return None;
    };
    let output = dir.join("out.mp4");
    Some((project, output))
}

#[test]
fn a_sequence_renders_to_a_file_with_progress_on_stderr() {
    let Some((project, output)) = ready("whole", false) else {
        return;
    };
    let (ok, stdout, stderr) = run(&[
        "render",
        project.to_str().expect("a utf-8 path"),
        "--sequence",
        "Main",
        "--preset",
        "youtube-1080p",
        "--out",
        output.to_str().expect("a utf-8 path"),
        "--encoder",
        "x264enc",
        "--range",
        ":5",
        "--verify",
    ]);
    assert!(ok, "render failed: {stderr}\n{stdout}");

    let report: serde_json::Value = serde_json::from_str(&stdout).expect("render prints JSON");
    assert_eq!(report["video_frames"], 5);
    assert_eq!(report["sequence"], "Main");
    assert_eq!(report["preset"], "youtube-1080p");
    assert_eq!(report["video_encoder"], "x264enc");
    assert_eq!(report["settings"]["width"], CANVAS.0);
    assert_eq!(report["settings"]["height"], CANVAS.1);
    assert_eq!(report["settings"]["frame_rate"]["numerator"], 25);
    assert_eq!(report["settings"]["frame_rate"]["denominator"], 1);
    // No audio track on this sequence, so nothing was asked of the mixer.
    assert!(report["settings"]["audio_codec"].is_null());
    assert!(
        report["probe"]["video"][0]["width"] == CANVAS.0,
        "the discoverer disagrees with the report: {}",
        report["probe"],
    );

    assert!(output.is_file(), "the render wrote no file");
    assert!(
        std::fs::metadata(&output)
            .expect("the file is readable")
            .len()
            > 0,
        "the render wrote an empty file",
    );
    assert!(
        stderr.contains("render:") && stderr.contains("frames"),
        "no progress reached stderr: {stderr:?}",
    );
}

#[test]
fn a_range_trims_what_is_written_and_audio_is_mixed_in() {
    let Some((project, output)) = ready("range", true) else {
        return;
    };
    // A preset whose audio codec this machine can actually encode: the
    // hosted runners and this workspace's GStreamer set all carry FLAC,
    // while an AAC encoder is a per-machine licensing question.
    let output = output.with_extension("mkv");
    let (ok, stdout, stderr) = run(&[
        "render",
        project.to_str().expect("a utf-8 path"),
        "--preset",
        "mezzanine",
        "--out",
        output.to_str().expect("a utf-8 path"),
        "--range",
        "10:16",
        "--verify",
        "--compact",
    ]);
    assert!(ok, "render failed: {stderr}\n{stdout}");

    let report: serde_json::Value = serde_json::from_str(&stdout).expect("render prints JSON");
    assert_eq!(report["range"]["start_frame"], 10);
    assert_eq!(report["range"]["end_frame"], 16);
    assert_eq!(report["video_frames"], 6);
    assert_eq!(report["settings"]["audio_codec"], "flac");
    assert!(
        report["audio_frames"].as_u64().unwrap_or(0) > 0,
        "the mixer contributed no audio: {report}",
    );
    assert!(
        !report["probe"]["audio"]
            .as_array()
            .expect("the probe lists audio streams")
            .is_empty(),
        "the written file carries no audio stream: {}",
        report["probe"],
    );
}

#[test]
fn a_render_that_cannot_be_run_says_why_and_fails() {
    let dir = scratch("errors");
    let Some(project) = project(&dir, false) else {
        eprintln!("skipping: no generated fixtures; run scripts/gen-fixtures.sh");
        return;
    };
    let path = project.to_str().expect("a utf-8 path").to_owned();
    let output = dir.join("out.mp4");
    let out = output.to_str().expect("a utf-8 path").to_owned();

    for (args, code) in [
        (
            vec![
                "render",
                &path,
                "--sequence",
                "Nowhere",
                "--preset",
                "youtube-1080p",
                "--out",
                &out,
            ],
            "core.not_found",
        ),
        (
            vec!["render", &path, "--preset", "no-such-preset", "--out", &out],
            "export.preset_unknown",
        ),
        (
            vec![
                "render",
                &path,
                "--preset",
                "youtube-1080p",
                "--out",
                &out,
                "--encoder",
                "nosuchenc",
            ],
            "export.unknown_encoder",
        ),
        (
            vec![
                "render",
                &path,
                "--preset",
                "youtube-1080p",
                "--out",
                &out,
                "--range",
                "900:999",
            ],
            "core.invalid_argument",
        ),
    ] {
        let (ok, _stdout, stderr) = run(&args);
        assert!(!ok, "{args:?} was accepted");
        let error: serde_json::Value =
            serde_json::from_str(&stderr).unwrap_or_else(|_| panic!("{args:?}: {stderr}"));
        assert_eq!(error["code"], code, "{args:?} reported {error}");
    }
    assert!(!output.exists(), "a failed render left a file behind");
}
