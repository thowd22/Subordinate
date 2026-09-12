//! The assembled editor exporting a real file (TASK-135, docs/PLAN.md §5.5).
//!
//! `export_panel.rs` covers the panel's picture, `export_runner.rs` covers the
//! wiring between the panel and the job over synthetic frames. This is the
//! whole of it: the real [`SubordinateApp`], opened on the committed sample
//! project with the CC0 media `scripts/get-sample-media.sh` fetches, asked for
//! an export the way a click on Export asks for one, and the file it wrote
//! probed afterwards with the GStreamer discoverer — the library
//! `gst-discoverer-1.0` is a thin shell around, and the binary itself where
//! this machine has it.
//!
//! Two things are proved that nothing below this level can prove:
//!
//! - the picture and the sound really arrive. The frames are the compositor's
//!   full-resolution readback of the sample sequence and the samples are the
//!   mixer's offline render of its audio track, so the written file has a
//!   video stream of exactly the frames that were asked for and an audio
//!   stream beside it;
//! - the window keeps painting while it happens. The export runs on the job
//!   pool, and the harness counts the frames the window painted between the
//!   start of the export and its end: an export that decoded, composited or
//!   mixed on the UI thread would paint one frame and then stop.
//!
//! The test skips itself, saying why, when the sample media has not been
//! fetched, when the machine enumerates no wgpu adapter, or when it has no
//! usable H.264 encoder — the same three reasons the render and job suites
//! skip.

mod support;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use sub_export::{EncoderPreferences, ExportElements, PresetLibrary, element_is_usable};
use sub_model::{Project, Sequence};
use sub_ui::export_panel::{ExportAction, ExportRange, ExportStatus};
use sub_ui::{AppOptions, SubordinateApp};

/// The sequence the sample project opens on: 1280x720 at 25 fps, with a video
/// track, an overlay track and a music bed.
const SEQUENCE: &str = "Main cut";

/// That sequence's timebase, which is the rate a written file's duration is
/// read back as a frame count at.
const SEQUENCE_RATE: sub_time::Rational = sub_time::Rational::FPS_25;

/// The preset the export is asked for: Matroska, H.264 and FLAC.
///
/// FLAC because the audio is what the CLI equivalence test compares byte for
/// byte, and a lossy codec would only prove the two paths were similar.
const PRESET: &str = "mezzanine";

/// How many frames of the sequence are exported.
///
/// Enough to cover the head of the first clip and the fade-in of the music
/// bed, and few enough that a CRF-0 software encode of a 720p canvas finishes
/// in a test.
const FRAMES: i64 = 12;

/// How long the test waits for the export to finish before calling the
/// environment, not the code, broken.
const PATIENCE: Duration = Duration::from_mins(3);

/// The window this suite paints, in logical points.
const WINDOW_SIZE: egui::Vec2 = egui::vec2(1400.0, 900.0);

use eframe::egui;

/// A folder of this test's own, emptied first so a rerun starts clean.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sub-ui-export-e2e-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary folder");
    dir
}

/// The committed sample project's folder.
///
/// `CARGO_MANIFEST_DIR` is baked in at compile time, so a test binary built on
/// one machine and run on another - which is how the export matrix keeps
/// compilation off the GPU instances - would look for the sample project in a
/// path that does not exist there. `SUBORDINATE_SAMPLE_PROJECT` names the
/// folder instead when it is set.
fn sample_dir() -> PathBuf {
    match std::env::var("SUBORDINATE_SAMPLE_PROJECT") {
        Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
        _ => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sample-project"),
    }
}

/// The sample project copied into a folder of the test's own, media and all.
///
/// Copied rather than opened in place because opening a project starts the
/// autosave worker, whose snapshots live in a sidecar beside the file; and
/// media and all because a clip's path is relative to the project file, so
/// the media has to travel with it.
///
/// `None` when the media has not been fetched, which is a skip rather than a
/// failure on a fresh checkout.
fn sample_copy(name: &str) -> Option<PathBuf> {
    let source = sample_dir();
    let media = source.join("media");
    let clips: Vec<PathBuf> = std::fs::read_dir(&media)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "webm"))
        .collect();
    if clips.is_empty() {
        eprintln!(
            "skipping: no sample media in {}; run scripts/get-sample-media.sh",
            media.display()
        );
        return None;
    }
    let dir = temp_dir(name);
    std::fs::create_dir_all(dir.join("media")).expect("a media folder");
    std::fs::copy(source.join("demo.sub"), dir.join("demo.sub")).expect("the project copies");
    for clip in clips {
        let name = clip.file_name().expect("a file name");
        std::fs::copy(&clip, dir.join("media").join(name)).expect("the media copies");
    }
    Some(dir.join("demo.sub"))
}

/// Whether this machine can encode the preset over the sample sequence.
fn can_encode(sequence: &Sequence) -> bool {
    let library = PresetLibrary::builtin();
    let Ok(preset) = library.require(PRESET) else {
        panic!("the built-in preset {PRESET} is gone");
    };
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

/// The assembled editor, opened on `project`.
fn app_harness(project: &Path) -> Harness<'static, SubordinateApp> {
    let options = AppOptions {
        project: Some(project.to_path_buf()),
        ..AppOptions::default()
    };
    support::builder::<SubordinateApp>()
        .with_size(WINDOW_SIZE)
        .build_eframe(move |cc| SubordinateApp::new(cc, options).expect("the editor starts"))
}

/// The main sequence of `project`, by name.
fn sequence_of(project: &Project) -> &Sequence {
    project
        .sequences
        .iter()
        .find(|sequence| sequence.name == SEQUENCE)
        .expect("the sample project has a main sequence")
}

/// Sets the panel up for a [`FRAMES`]-frame export of the main sequence to
/// `output`, and returns the request a click on Export would build.
fn arrange_export(
    harness: &mut Harness<'_, SubordinateApp>,
    output: &Path,
) -> sub_ui::export_panel::ExportRequest {
    let project = harness.state().project().clone();
    let id = sequence_of(&project).id;
    let panel = harness.state_mut().export_panel();
    panel.select_preset(PRESET).expect("the preset is offered");
    panel.select_sequence(&project, id);
    panel.set_range(ExportRange::InToOut);
    panel.set_in_out(0, FRAMES);
    panel.set_output(output);
    assert_eq!(
        panel.unavailable(),
        None,
        "the Export button is held closed: {:?}",
        panel.unavailable()
    );
    panel.request(&project).expect("the panel builds a request")
}

/// Paints frames until the export is over, and reports how many it painted.
///
/// This is the "does not block the window" assertion's measurement: every turn
/// of this loop is a whole frame of the real window, laid out and painted.
fn run_until_settled(harness: &mut Harness<'_, SubordinateApp>) -> u32 {
    let deadline = Instant::now() + PATIENCE;
    let mut painted = 0;
    loop {
        // `step`, not `run`: the window asks for a repaint every 100 ms for as
        // long as an export is running, which is exactly the condition
        // `Harness::run` refuses to loop on.
        harness.step();
        painted += 1;
        match harness.state().export_status() {
            ExportStatus::Idle | ExportStatus::Running { .. } => {}
            _ => return painted,
        }
        assert!(
            Instant::now() < deadline,
            "the export did not finish within {PATIENCE:?}; painted {painted} frames"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_window_exports_the_sample_project_to_a_playable_file() {
    if !support::can_render() {
        return;
    }
    let Some(path) = sample_copy("window") else {
        return;
    };
    let mut harness = app_harness(&path);
    support::run_settled(&mut harness);
    let project = harness.state().project().clone();
    if !can_encode(sequence_of(&project)) {
        return;
    }

    let output = path.with_file_name("window-export.mkv");
    let request = arrange_export(&mut harness, &output);
    // The same action the Export button hands back, applied the same way.
    harness
        .state_mut()
        .apply_export(ExportAction::Start(Box::new(request)));

    let painted = run_until_settled(&mut harness);
    let report = match harness.state().export_status() {
        ExportStatus::Finished(report) => report.clone(),
        ExportStatus::Failed { error, .. } => {
            panic!("the export failed: [{}] {}", error.code, error.message)
        }
        other => panic!("the export neither finished nor failed: {other:?}"),
    };

    assert_eq!(
        report.video_frames,
        u64::try_from(FRAMES).expect("a small frame count"),
        "the export wrote a different number of frames than it was asked for"
    );
    assert!(output.is_file(), "{} was not written", output.display());

    // An export that decoded, composited or mixed on the UI thread would have
    // painted the frame that started it and then nothing until it was over.
    eprintln!(
        "the window painted {painted} frames while the export wrote {} frames",
        report.video_frames
    );
    assert!(
        painted > 2,
        "the window painted only {painted} frames while the export ran; \
         the export is blocking the UI thread"
    );

    // The GStreamer discoverer — what `gst-discoverer-1.0` is a shell around —
    // on the file that was actually written.
    let info = sub_media::probe::probe(&output).expect("the written file probes");
    assert_eq!(info.video.len(), 1, "one video stream");
    let video = &info.video[0];
    assert_eq!(
        (video.width, video.height),
        (1280, 720),
        "the canvas the sequence composites at is the canvas that was written"
    );
    assert!(
        info.has_audio(),
        "the sequence's music bed was not written: {:?}",
        info.audio
    );

    let rate = sequence_of(&project).settings.frame_rate;
    let duration = info.duration.expect("the file has a duration");
    let frames = duration
        .rescaled_to_rounding(rate, sub_time::Rounding::Nearest)
        .value();
    assert_eq!(
        frames, FRAMES,
        "the discoverer found {frames} frames' worth of {duration}, not {FRAMES}"
    );

    // Where the binary is installed, it says the same thing; where it is not,
    // the library above has already said it.
    if let Some(text) = discoverer_binary(&output) {
        assert!(
            text.contains("video") || text.contains("Video"),
            "gst-discoverer-1.0 found no video stream:\n{text}"
        );
        assert!(
            text.contains("audio") || text.contains("Audio"),
            "gst-discoverer-1.0 found no audio stream:\n{text}"
        );
    }
}

#[test]
fn an_unsaved_project_says_why_it_cannot_be_exported() {
    if !support::can_render() {
        return;
    }
    // A window with no project file: its clips would name media relative to a
    // folder that does not exist, so the Export button is held closed rather
    // than starting an export that could only fail.
    let mut harness = support::builder::<SubordinateApp>()
        .with_size(WINDOW_SIZE)
        .build_eframe(|cc| {
            SubordinateApp::new(cc, AppOptions::default()).expect("the editor starts")
        });
    support::run_settled(&mut harness);
    assert_eq!(
        harness.state_mut().export_panel().unavailable(),
        Some(sub_ui::app::NO_RENDERER_REASON),
        "an unsaved project offers an Export button that could only fail"
    );
}

/// The environment variable naming the encoders the GUI-versus-CLI comparison
/// runs for, comma separated, and the one naming the `subordinate-cli` it
/// compares against.
///
/// Both are absent everywhere but the export matrix workflow, and the test
/// below skips itself when either is: hosted CI has no hardware encoder to
/// compare and no reason to run a render twice.
const MATRIX_ENCODERS_ENV: &str = "SUBORDINATE_MATRIX_ENCODERS";
const MATRIX_CLI_ENV: &str = "SUBORDINATE_CLI";

/// The window's export and `subordinate-cli render` write the same file.
///
/// TASK-135 connected the editor's export to the compositor readback and the
/// offline mix, and TASK-143 is where that claim is finally tested against the
/// other path on real hardware: the same project, the same preset, the same
/// pinned encoder, exported once by the assembled window and once by the CLI,
/// and the two files compared on what an export is for - how many frames came
/// back out of it and whether the sound is there.
///
/// The encoder is pinned on both sides rather than left to the automatic
/// order, because the interesting comparison is per encoder: a GUI export that
/// silently landed on `x264enc` while the CLI used NVENC would compare two
/// different things and pass.
#[test]
fn the_window_and_the_cli_write_the_same_file_for_each_encoder() {
    if !support::can_render() {
        return;
    }
    let Ok(encoders) = std::env::var(MATRIX_ENCODERS_ENV) else {
        eprintln!("skipping: {MATRIX_ENCODERS_ENV} names no encoders to compare");
        return;
    };
    let cli = match std::env::var(MATRIX_CLI_ENV) {
        Ok(path) if Path::new(&path).is_file() => PathBuf::from(path),
        _ => {
            eprintln!("skipping: {MATRIX_CLI_ENV} does not name a subordinate-cli binary");
            return;
        }
    };
    let Some(path) = sample_copy("gui-vs-cli") else {
        return;
    };

    let probe = sub_export::EncoderProbe::cached().expect("an encoder probe");
    for element in encoders.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let pinnable = probe
            .encoders
            .iter()
            .any(|status| status.element == element && status.is_pinnable());
        if pinnable {
            compare_window_and_cli(&path, &cli, element);
        } else {
            eprintln!("skipping {element}: this machine cannot start it");
        }
    }
}

/// The window's export of `path` with `element` pinned, against the CLI's.
///
/// Panics with what differed, which is what makes it a test: a frame the
/// window wrote and the CLI did not, or sound in one file and not the other,
/// is the whole failure mode TASK-135 could not rule out.
fn compare_window_and_cli(path: &Path, cli: &Path, element: &str) {
    let gui_output = path.with_file_name(format!("gui-{element}.mkv"));
    let cli_output = path.with_file_name(format!("cli-{element}.mkv"));
    let gui = window_export(path, &gui_output, element);
    assert_eq!(
        gui.video_encoder, element,
        "the window exported with {} rather than the pinned {element}",
        gui.video_encoder
    );
    cli_render(cli, path, &cli_output, element);

    let gui_info = sub_media::probe::probe(&gui_output).expect("the window's file probes");
    let cli_info = sub_media::probe::probe(&cli_output).expect("the CLI's file probes");
    let (gui_frames, cli_frames) = (frames_of(&gui_info), frames_of(&cli_info));
    assert_eq!(
        gui_frames, cli_frames,
        "{element}: the window wrote {gui_frames} frames and the CLI wrote {cli_frames}",
    );
    assert_eq!(
        gui_frames, FRAMES,
        "{element}: neither path wrote the frames it was asked for"
    );
    assert_eq!(
        gui_info.has_audio(),
        cli_info.has_audio(),
        "{element}: only one of the two paths wrote audio"
    );
    assert!(
        gui_info.has_audio(),
        "{element}: neither path wrote the sequence's audio"
    );
    eprintln!("{element}: window {gui_frames} frames, CLI {cli_frames} frames, audio on both");
}

/// Exports `frames` of the sample project through the assembled window, with
/// `element` pinned in the export panel exactly as the encoder picker pins one.
fn window_export(project: &Path, output: &Path, element: &str) -> sub_export::ExportReport {
    let mut harness = app_harness(project);
    support::run_settled(&mut harness);
    let project = harness.state().project().clone();
    let id = sequence_of(&project).id;
    {
        let panel = harness.state_mut().export_panel();
        panel.select_preset(PRESET).expect("the preset is offered");
        panel.select_sequence(&project, id);
        panel.set_range(ExportRange::InToOut);
        panel.set_in_out(0, FRAMES);
        panel.set_output(output);
        panel
            .set_encoder_override(Some(element))
            .expect("the element is catalogued");
    }
    let request = harness
        .state_mut()
        .export_panel()
        .request(&project)
        .expect("the panel builds a request");
    harness
        .state_mut()
        .apply_export(ExportAction::Start(Box::new(request)));
    run_until_settled(&mut harness);
    match harness.state().export_status() {
        ExportStatus::Finished(report) => report.clone(),
        ExportStatus::Failed { error, .. } => {
            panic!(
                "the window's export with {element} failed: [{}] {}",
                error.code, error.message
            )
        }
        other => panic!("the window's export with {element} did not finish: {other:?}"),
    }
}

/// The same frames of the same project through `subordinate-cli render`.
fn cli_render(cli: &Path, project: &Path, output: &Path, element: &str) {
    let render = std::process::Command::new(cli)
        .args([
            "render",
            &project.display().to_string(),
            "--sequence",
            SEQUENCE,
            "--preset",
            PRESET,
            "--encoder",
            element,
            "--range",
            &format!("0:{FRAMES}"),
            "--out",
            &output.display().to_string(),
            "--compact",
        ])
        .output()
        .expect("subordinate-cli runs");
    assert!(
        render.status.success(),
        "subordinate-cli render with {element} failed: {}",
        String::from_utf8_lossy(&render.stderr)
    );
}

/// How many frames of the sample sequence's timebase a probed file lasts.
fn frames_of(info: &sub_media::probe::MediaInfo) -> i64 {
    info.duration
        .expect("the file has a duration")
        .rescaled_to_rounding(SEQUENCE_RATE, sub_time::Rounding::Nearest)
        .value()
}

/// `gst-discoverer-1.0`'s own report on `path`, when the binary is installed.
fn discoverer_binary(path: &Path) -> Option<String> {
    let output = std::process::Command::new("gst-discoverer-1.0")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        eprintln!(
            "gst-discoverer-1.0 refused {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}
