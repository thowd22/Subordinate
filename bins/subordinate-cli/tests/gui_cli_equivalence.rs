//! A GUI export and a `subordinate-cli render` of the same project are the
//! same render (TASK-135, docs/PLAN.md §5.5).
//!
//! Both write a file through `sub_export::sequence`: the compositor's
//! full-resolution readback (TASK-58) for the picture and the mixer's offline
//! render (TASK-55) for the sound. That is the point of putting those adapters
//! in one crate rather than one copy per front end, and this is the test that
//! keeps it true. A second copy of either — a different rounding rule for the
//! frame count, a different resample order, a canvas taken from the preset on
//! one side and the sequence on the other — would show up here as a different
//! frame count or a different sample.
//!
//! The GUI side is not a mock. It is `sub_ui`'s own [`ExportRunner`] over
//! `sub_ui::export_runner::sequence_sources`, spawned on a `JobService`, with
//! the request built by the real [`ExportPanel`] — exactly what the editor
//! window runs when Export is clicked, minus the window.
//!
//! The audio comparison is byte-for-byte on the decoded samples, which is why
//! the preset is FLAC: a lossy codec could only show that the two paths were
//! similar.
//!
//! The test skips itself, saying why, when the fixtures have not been
//! generated, when this machine enumerates no wgpu adapter, or when no usable
//! H.264 encoder is installed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sub_core::JobService;
use sub_export::{EncoderPreferences, ExportElements, PresetLibrary, element_is_usable};
use sub_model::{
    Clip, ColorTags, MediaItem, MediaPath, Project, Resolution, Sequence, SequenceSettings, Track,
    TrackKind, json,
};
use sub_render::RenderContext;
use sub_time::{Rational, RationalTime, TimeRange};
use sub_ui::export_panel::{ExportPanel, ExportRange, ExportStatus};
use sub_ui::export_runner::{ExportRunner, sequence_sources};

/// The sequence timebase, which is the rate of both fixtures below.
const FPS: Rational = Rational::FPS_25;
/// How long the clips are, in frames.
const CLIP_FRAMES: i64 = 50;
/// The canvas: small enough to encode in a moment on a software adapter.
const CANVAS: (u32, u32) = (320, 240);
/// The frames both sides render: the head of the clips, and a range rather
/// than the whole sequence so the range arithmetic is compared too.
const FRAMES: i64 = 10;
/// Matroska, H.264 and FLAC — the audio lossless, so two renders that agree
/// agree exactly.
const PRESET: &str = "mezzanine";
/// The sequence both sides name.
const SEQUENCE: &str = "Main";

/// How long the GUI-side export may take before the environment, not the
/// code, is at fault.
const PATIENCE: Duration = Duration::from_mins(3);

/// The fixture carrying the picture.
const VIDEO_FIXTURE: &str = "bars_1080p_h264.mp4";
/// The fixture carrying the sound.
const AUDIO_FIXTURE: &str = "tone_48k_stereo.wav";

/// A scratch directory of its own for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("subordinate-cli-equivalence-{name}"));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(dir.join("media")).expect("a scratch directory");
    dir
}

/// Copies a generated fixture into the project's own `media` folder, because a
/// project file may only name paths relative to itself.
fn media(dir: &Path, name: &str) -> Option<MediaPath> {
    let source = sub_test_support::try_fixture(name)?;
    let relative = format!("media/{name}");
    std::fs::copy(&source, dir.join(&relative)).expect("the fixture copies into the project");
    Some(MediaPath::new(&relative).expect("a project-relative media path"))
}

/// A project holding one video clip and one audio clip, written to `dir`.
fn project(dir: &Path) -> Option<(PathBuf, Project)> {
    let settings = SequenceSettings::new(
        Resolution::new(CANVAS.0, CANVAS.1).expect("a valid canvas"),
        FPS,
        48_000,
        ColorTags::REC709,
    )
    .expect("valid sequence settings");
    let mut project = Project::new("equivalence");
    let mut sequence = Sequence::new(SEQUENCE, settings);

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

    project.sequences.push(sequence);
    let path = dir.join("equivalence.sub");
    std::fs::write(
        &path,
        json::to_json(&project).expect("the project serialises"),
    )
    .expect("the project file writes");
    Some((path, project))
}

/// Whether this machine can encode the preset over `sequence`.
fn can_encode(sequence: &Sequence) -> bool {
    if sub_render::RenderContext::headless().is_err() {
        eprintln!("skipping: this machine enumerates no wgpu adapter");
        return false;
    }
    let library = PresetLibrary::builtin();
    let preset = library.require(PRESET).expect("the built-in preset");
    let Ok((settings, _)) = sub_export::settings_for_sequence(preset, sequence) else {
        return false;
    };
    if sub_export::EncoderProbe::cached().is_err() {
        eprintln!("skipping: GStreamer has no usable encoder catalogue here");
        return false;
    }
    let usable = ExportElements::resolve(&settings, &EncoderPreferences::new()).is_ok()
        && element_is_usable(settings.container.muxer());
    if !usable {
        eprintln!("skipping: this machine has no usable encoder or muxer for {PRESET}");
    }
    usable
}

/// Renders `frames` of the project through `subordinate-cli render`, and
/// hands back the report it printed.
fn cli_render(project: &Path, output: &Path) -> serde_json::Value {
    let result = Command::new(env!("CARGO_BIN_EXE_subordinate-cli"))
        .args([
            "render",
            &project.display().to_string(),
            "--preset",
            PRESET,
            "--sequence",
            SEQUENCE,
            "--out",
            &output.display().to_string(),
            "--range",
            &format!("0:{FRAMES}"),
        ])
        .env("SUBORDINATE_LOG", "warn")
        .env_remove("RUST_LOG")
        .output()
        .expect("subordinate-cli runs");
    assert!(
        result.status.success(),
        "subordinate-cli render failed:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).expect("the render report is JSON")
}

/// Renders the same frames the way the editor window does: the panel builds
/// the request, the runner spawns the job on the worker pool, and the streams
/// come from the shared sequence adapters.
fn gui_render(project: &Project, project_dir: &Path, output: &Path) -> sub_export::ExportReport {
    let library = PresetLibrary::builtin();
    let mut panel = ExportPanel::new();
    panel.set_library(&library);
    panel.select_preset(PRESET).expect("the preset is offered");
    panel.select_sequence(project, project.sequences[0].id);
    panel.set_range(ExportRange::InToOut);
    panel.set_in_out(0, FRAMES);
    panel.set_output(output);
    let request = panel.request(project).expect("the panel builds a request");

    let render = RenderContext::headless().expect("a headless adapter");
    let jobs = JobService::new(1);
    let mut runner = ExportRunner::new();
    runner
        .start(
            &jobs,
            &library,
            project,
            &request,
            &mut sequence_sources(
                &render,
                Arc::new(project.clone()),
                project_dir.to_path_buf(),
            ),
        )
        .expect("the export starts");

    let deadline = Instant::now() + PATIENCE;
    loop {
        runner.poll(&mut panel);
        match panel.status() {
            ExportStatus::Finished(report) => return (**report).clone(),
            ExportStatus::Failed { error, .. } => {
                panic!(
                    "the GUI-path export failed: [{}] {}",
                    error.code, error.message
                )
            }
            ExportStatus::Cancelled { .. } => panic!("nothing cancelled the export"),
            ExportStatus::Idle | ExportStatus::Running { .. } => {}
        }
        assert!(
            Instant::now() < deadline,
            "the GUI-path export did not finish within {PATIENCE:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_gui_export_and_a_cli_render_write_the_same_frames_and_the_same_audio() {
    let dir = scratch("same");
    let Some((path, project)) = project(&dir) else {
        eprintln!("skipping: the media fixtures have not been generated");
        return;
    };
    if !can_encode(&project.sequences[0]) {
        return;
    }

    let cli_out = dir.join("cli.mkv");
    let gui_out = dir.join("gui.mkv");
    let cli = cli_render(&path, &cli_out);
    let gui = gui_render(&project, &dir, &gui_out);

    // The same number of frames, and the number that was asked for.
    let expected = u64::try_from(FRAMES).expect("a small frame count");
    assert_eq!(
        gui.video_frames, expected,
        "the GUI export wrote {expected}"
    );
    assert_eq!(
        cli["video_frames"].as_u64(),
        Some(expected),
        "the CLI render wrote {expected}: {cli}"
    );

    // The same settings: the canvas and the timebase both sides took from the
    // sequence rather than from the preset.
    assert_eq!(cli["settings"]["width"].as_u64(), Some(u64::from(CANVAS.0)));
    assert_eq!(
        cli["settings"]["height"].as_u64(),
        Some(u64::from(CANVAS.1))
    );
    assert_eq!(
        gui.video_encoder, cli["video_encoder"],
        "the same encoder element ran on both sides"
    );
    assert_eq!(
        gui.duration.value(),
        i64::from(i32::try_from(FRAMES).expect("small"))
    );

    // The same sound, sample for sample. FLAC is lossless, so two identical
    // mixes decode back to identical samples and two different ones do not.
    let from_cli = sub_audio::decode::decode_file(&cli_out).expect("the CLI file's audio decodes");
    let from_gui = sub_audio::decode::decode_file(&gui_out).expect("the GUI file's audio decodes");
    assert_eq!(from_cli.sample_rate, from_gui.sample_rate);
    assert_eq!(from_cli.channels, from_gui.channels);
    assert_eq!(
        from_cli.samples.len(),
        from_gui.samples.len(),
        "the two renders mixed a different number of samples"
    );
    assert!(
        from_cli
            .samples
            .iter()
            .zip(&from_gui.samples)
            .all(|(a, b)| a.to_bits() == b.to_bits()),
        "the two renders mixed the same span to different samples"
    );
    assert!(
        from_gui.samples.iter().any(|sample| *sample != 0.0),
        "both renders produced silence, which would agree for the wrong reason"
    );

    // And the file each wrote really is what the discoverer says it is.
    for written in [&cli_out, &gui_out] {
        let info = sub_media::probe::probe(written).expect("the written file probes");
        assert_eq!(
            info.video.len(),
            1,
            "{} has one video stream",
            written.display()
        );
        assert!(info.has_audio(), "{} has audio", written.display());
    }
}
