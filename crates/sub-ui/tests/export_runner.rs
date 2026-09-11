//! The export panel driving a real export job (TASK-62, docs/PLAN.md §5.5).
//!
//! `export_panel.rs` covers the panel's picture and the request a click on
//! Export builds. This covers what happens next: an [`ExportRunner`] turns
//! that request into a [`sub_export`] job on the shared worker pool, and every
//! number the panel then shows — the progress bar's fraction, the percentage,
//! the frame count and the ETA — comes back out of the running encoder rather
//! than out of the panel's imagination. The Cancel button is tested the same
//! way: it stops the real export, and the part-written file is gone afterwards.
//!
//! The frames are synthetic, because the compositor is not what is under test
//! here; they arrive through the same [`ExportStreams`] hook the host will use
//! for the compositor's readback. A machine with no usable H.264 encoder skips
//! these tests, exactly as the `sub-export` job tests do.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sub_core::{JobService, SubError, SubResult};
use sub_export::{
    EncoderPreferences, ExportElements, ExportSettings, PresetLibrary, SolidFrames,
    VideoFrameSource, element_is_usable,
};
use sub_model::sequence::SequenceSettings;
use sub_model::{ColorTags, Project, Resolution, Sequence};
use sub_time::{Rational, RationalTime, TimeRange};
use sub_ui::export_panel::{ExportPanel, ExportRange, ExportRequest, ExportStatus, PresetEntry};
use sub_ui::export_runner::{ExportRunner, ExportSources, ExportStreams, settings_for};

/// How long a test waits for a worker thread to get somewhere before it calls
/// the environment, not the code, broken.
const PATIENCE: Duration = Duration::from_secs(30);

/// A preset small enough to encode in a moment: 64×48 silent Matroska, which
/// is the cheapest real pipeline the exporter can build.
const SMALL_PRESET: &str = r#"
[[preset]]
id = "tiny-mkv"
name = "Tiny Matroska"
container = "mkv"
video_codec = "h264"
width = 64
height = 48
frame_rate_numerator = 24
frame_rate_denominator = 1
video_crf = 30
"#;

/// The library the panel offers in these tests.
fn library() -> PresetLibrary {
    PresetLibrary::from_toml(SMALL_PRESET).expect("the test preset parses")
}

/// A project whose only sequence is the 64x48 canvas the test preset asks
/// for, and the request a panel showing that preset would build for `frames`
/// frames of it written to `output`.
///
/// The settings an export resolves come from the sequence's canvas and
/// timebase, so the request only means anything beside the project it names.
fn request(output: &Path, frames: i64) -> (Project, ExportRequest) {
    let library = library();
    let preset = library.require("tiny-mkv").expect("the test preset");
    let settings = SequenceSettings::new(
        Resolution::new(64, 48).expect("a valid canvas"),
        Rational::FPS_24,
        48_000,
        ColorTags::default(),
    )
    .expect("valid sequence settings");
    let sequence = Sequence::new("Main", settings);
    let id = sequence.id;
    let mut project = Project::new("export-runner");
    project.sequences.push(sequence);
    let request = ExportRequest {
        preset: PresetEntry::from_preset(preset),
        sequence: id,
        range: ExportRange::WholeSequence,
        span: TimeRange::new(
            RationalTime::from_frames(0, Rational::FPS_24),
            RationalTime::from_frames(frames, Rational::FPS_24),
        )
        .expect("a non-negative span"),
        output: output.to_path_buf(),
        encoder_override: None,
    };
    (project, request)
}

/// Whether this machine can encode the test preset at all.
fn can_export(settings: &ExportSettings) -> bool {
    if sub_export::EncoderProbe::cached().is_err() {
        return false;
    }
    ExportElements::resolve(settings, &EncoderPreferences::new()).is_ok()
        && element_is_usable(settings.container.muxer())
}

/// A fresh temporary directory named after the test using it.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sub-ui-export-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    dir
}

/// Black frames that stop after `gate_after` of them until the test opens the
/// gate.
///
/// This is what makes the test deterministic rather than timed: the export is
/// held mid-render, with its first progress event already posted, for exactly
/// as long as the test needs to look at the panel or press Cancel.
struct GatedFrames {
    inner: SolidFrames,
    handed_out: u64,
    gate_after: u64,
    open: Arc<AtomicBool>,
}

impl GatedFrames {
    fn new(settings: &ExportSettings, frames: u64, gate_after: u64) -> (Self, Arc<AtomicBool>) {
        let open = Arc::new(AtomicBool::new(false));
        (
            Self {
                inner: SolidFrames::new(settings.frame_bytes(), frames),
                handed_out: 0,
                gate_after,
                open: Arc::clone(&open),
            },
            open,
        )
    }
}

impl VideoFrameSource for GatedFrames {
    fn next_frame(&mut self) -> SubResult<Option<&[u8]>> {
        if self.handed_out == self.gate_after {
            let deadline = Instant::now() + PATIENCE;
            while !self.open.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        self.handed_out += 1;
        self.inner.next_frame()
    }
}

/// A source hook that hands its streams over once, which is what a host does
/// per export.
struct Once(Option<ExportStreams>);

impl ExportSources for Once {
    fn open(&mut self, _: &ExportRequest, _: &ExportSettings) -> SubResult<ExportStreams> {
        self.0.take().ok_or_else(|| {
            SubError::new(
                sub_core::codes::INVALID_STATE,
                "the streams were already handed over",
            )
        })
    }
}

/// Polls `runner` into `panel` until `ready` is happy with the panel, and
/// returns whether it ever was.
fn poll_until(
    runner: &mut ExportRunner,
    panel: &mut ExportPanel,
    ready: impl Fn(&ExportPanel) -> bool,
) -> bool {
    let deadline = Instant::now() + PATIENCE;
    loop {
        runner.poll(panel);
        if ready(panel) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_running_export_fills_the_panels_progress_bar_and_eta() {
    let library = library();
    let dir = temp_dir("progress");
    let path = dir.join("progress.mkv");
    let (project, request) = request(&path, 24);
    let settings = settings_for(&library, &project, &request).expect("the preset resolves");
    if !can_export(&settings) {
        eprintln!("skipping: this machine has no usable H.264 encoder or matroskamux");
        return;
    }

    let jobs = JobService::new(1);
    let mut panel = ExportPanel::new();
    panel.set_library(&library);
    let mut runner = ExportRunner::new();
    let (frames, gate) = GatedFrames::new(&settings, 24, 1);
    let mut sources = Once(Some(ExportStreams::video(Box::new(frames))));
    runner
        .start(&jobs, &library, &project, &request, &mut sources)
        .expect("the export starts");
    assert!(runner.is_running(), "the runner owns the job it queued");

    // The panel is told the export started, and then how it is going, without
    // the test ever touching the panel's own state.
    assert!(
        poll_until(&mut runner, &mut panel, |panel| matches!(
            panel.status(),
            ExportStatus::Running {
                progress: Some(_),
                ..
            }
        )),
        "the first progress snapshot reaches the panel"
    );
    let ExportStatus::Running {
        path: running_path,
        frames_total,
        progress: Some(progress),
        stats,
    } = panel.status().clone()
    else {
        panic!("the panel is showing a running export");
    };
    assert_eq!(running_path, path, "the panel names the file being written");
    assert_eq!(frames_total, 24, "the job was told how long the export is");
    assert_eq!(progress.frames_total, 24);
    assert!(progress.frames_done >= 1);
    assert!(
        progress.percent_milli.is_some(),
        "a known length means a percentage"
    );
    assert!(
        progress.eta.is_some(),
        "and a percentage that is moving means an ETA"
    );
    assert!(
        !stats.video_encoder.is_empty(),
        "the panel knows which encoder is running"
    );
    let fraction = panel.status().fraction().expect("a bar to draw");
    assert!(
        (0.0..=1.0).contains(&fraction),
        "the bar's fraction stays on the bar: {fraction}"
    );
    let summary = panel.status().summary();
    assert!(
        summary.contains("of 24 frames") && summary.contains("left"),
        "the status line carries the frame count and the ETA: {summary}"
    );

    gate.store(true, Ordering::Release);
    assert!(
        poll_until(&mut runner, &mut panel, |panel| matches!(
            panel.status(),
            ExportStatus::Finished(_)
        )),
        "the export finishes and the panel is told"
    );
    assert!(!runner.is_running(), "and the runner is free again");
    let ExportStatus::Finished(report) = panel.status().clone() else {
        panic!("the export finished");
    };
    assert_eq!(report.video_frames, 24);
    assert_eq!(report.path, path);
    assert!(path.is_file(), "a finished export leaves its file");
    assert_eq!(
        panel.status().fraction(),
        Some(1.0),
        "a finished export fills the bar"
    );
    let recent = panel
        .recent()
        .first()
        .expect("the recent list remembers it");
    assert_eq!(recent.path, path);
    assert_eq!(recent.frames, 24);
    jobs.wait_idle();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_cancel_button_stops_the_running_export_and_removes_the_part_file() {
    let library = library();
    let dir = temp_dir("cancel");
    let path = dir.join("cancel.mkv");
    // Far more frames than the test will let through: the export is stopped,
    // never finished.
    let (project, request) = request(&path, 10_000);
    let settings = settings_for(&library, &project, &request).expect("the preset resolves");
    if !can_export(&settings) {
        eprintln!("skipping: this machine has no usable H.264 encoder or matroskamux");
        return;
    }

    let jobs = JobService::new(1);
    let mut panel = ExportPanel::new();
    panel.set_library(&library);
    let mut runner = ExportRunner::new();
    let (frames, gate) = GatedFrames::new(&settings, 10_000, 1);
    let mut sources = Once(Some(ExportStreams::video(Box::new(frames))));
    runner
        .start(&jobs, &library, &project, &request, &mut sources)
        .expect("the export starts");

    assert!(
        poll_until(&mut runner, &mut panel, |panel| panel.status().is_running()),
        "the export is running before it is cancelled"
    );

    // Exactly what the panel's Cancel button raises, through the same runner
    // the host holds.
    runner.cancel();
    gate.store(true, Ordering::Release);

    assert!(
        poll_until(&mut runner, &mut panel, |panel| matches!(
            panel.status(),
            ExportStatus::Cancelled { .. }
        )),
        "the panel is told the export was cancelled"
    );
    let ExportStatus::Cancelled { frames_done } = panel.status().clone() else {
        panic!("the export was cancelled");
    };
    assert!(
        frames_done < 10_000,
        "a cancel stops the export early, not at the end: {frames_done}"
    );
    assert!(!runner.is_running(), "the runner is free after a cancel");
    assert!(
        !path.exists(),
        "a cancelled export leaves no half-written file wearing a real name"
    );
    assert!(
        panel.recent().is_empty(),
        "and nothing is remembered as exported"
    );
    assert!(panel.status().fraction().is_none(), "and no bar is drawn");
    jobs.wait_idle();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_export_that_cannot_start_reports_the_element_that_refused() {
    let library = library();
    let dir = temp_dir("refused");
    let (project, mut request) = request(&dir.join("refused.mkv"), 4);
    // An encoder this machine certainly does not have, pinned the way the
    // panel's encoder picker pins one.
    request.encoder_override = Some("nvh264enc".to_owned());
    let settings = settings_for(&library, &project, &request).expect("the preset resolves");
    assert_eq!(settings.width, 64);
    // The probe initialises GStreamer, which asking after an element needs.
    if !can_export(&settings) {
        eprintln!("skipping: this machine has no usable H.264 encoder or matroskamux");
        return;
    }
    if element_is_usable("nvh264enc") {
        eprintln!("skipping: this machine really does have nvh264enc");
        return;
    }

    let jobs = JobService::new(1);
    let mut runner = ExportRunner::new();
    let mut sources = Once(Some(ExportStreams::video(Box::new(SolidFrames::new(
        settings.frame_bytes(),
        4,
    )))));
    let error = runner
        .start(&jobs, &library, &project, &request, &mut sources)
        .expect_err("an unavailable encoder stops the export before it starts");
    assert_eq!(error.code, sub_export::codes::ENCODER_UNAVAILABLE);
    assert!(
        !runner.is_running(),
        "a refused start leaves the runner free"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
