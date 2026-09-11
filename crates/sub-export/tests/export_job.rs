//! The export job end to end: the events a watcher sees, what a cancel leaves
//! on disk, and what a failure says about which element broke (TASK-61,
//! docs/PLAN.md §5.5).
//!
//! Every test that needs a real encoder skips itself when the machine has
//! none, exactly as the round-trip tests do; the cancellation and error tests
//! then still say something, because they are about the job loop rather than
//! the picture that comes out of it.

use std::path::PathBuf;
use std::time::Duration;

use sub_core::SubResult;
use sub_core::jobs::{CancelToken, JobOutcome, JobService, Priority};
use sub_export::{
    Container, ExportElements, ExportEvent, ExportJob, ExportSettings, SolidFrames, VideoCodec,
    VideoFrameSource, element_is_usable, spawn_export_job,
};
use sub_time::Rational;

/// Canvas of the test exports: small enough to encode in a moment.
const WIDTH: u32 = 64;
/// Canvas height.
const HEIGHT: u32 = 48;

/// Settings for a video-only Matroska export, which is the cheapest real
/// pipeline this crate can build.
fn settings() -> ExportSettings {
    ExportSettings::new(WIDTH, HEIGHT, Rational::FPS_24, Container::Mkv)
        .with_video_codec(VideoCodec::H264)
        .with_audio_codec(None)
}

/// The elements for `settings`, or `None` when this machine cannot encode.
fn can_export(settings: &ExportSettings) -> Option<ExportElements> {
    if sub_export::EncoderProbe::cached().is_err() {
        return None;
    }
    let elements =
        ExportElements::resolve(settings, &sub_export::EncoderPreferences::new()).ok()?;
    element_is_usable(settings.container.muxer()).then_some(elements)
}

/// A fresh temporary directory named after the test using it.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sub-export-job-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    dir
}

/// Black frames that cancel `token` once `cancel_after` of them have been
/// handed out.
///
/// Cancelling from inside the source is what a person pressing the button
/// mid-export looks like to the job loop, without the test having to guess how
/// long an encoder takes. The frame already in hand when the cancel arrives is
/// still finished, so the export writes one more than `cancel_after`.
struct CancellingFrames {
    inner: SolidFrames,
    token: CancelToken,
    cancel_after: u64,
    handed_out: u64,
}

impl VideoFrameSource for CancellingFrames {
    fn next_frame(&mut self) -> SubResult<Option<&[u8]>> {
        if self.handed_out == self.cancel_after {
            self.token.cancel();
        }
        self.handed_out += 1;
        self.inner.next_frame()
    }
}

#[test]
fn a_job_reports_frames_an_eta_and_encoder_stats_then_finishes() {
    let settings = settings();
    let Some(elements) = can_export(&settings) else {
        eprintln!("skipping: this machine has no usable H.264 encoder or matroskamux");
        return;
    };
    let dir = temp_dir("progress");
    let path = dir.join("progress.mkv");

    let frames = 24;
    let job = ExportJob::new(&path, &settings, &elements)
        .with_total_frames(frames)
        // Every frame reports, so the test reads the whole sequence rather
        // than whatever a timer happened to let through.
        .with_progress_interval(Duration::ZERO);

    let mut source = SolidFrames::new(settings.frame_bytes(), frames);
    let mut events = Vec::new();
    let report = job
        .run(&mut source, None, &mut |event| events.push(event.clone()))
        .expect("the export runs");

    assert_eq!(report.video_frames, frames);
    assert!(path.is_file(), "a finished export leaves its file");

    let ExportEvent::Started {
        path: started_path,
        frames_total,
        stats,
    } = &events[0]
    else {
        panic!("the first event is Started, got {:?}", events[0]);
    };
    assert_eq!(started_path, &path);
    assert_eq!(*frames_total, frames);
    assert_eq!(stats.video_encoder, elements.video_encoder);
    assert_eq!(stats.muxer, "matroskamux");
    assert_eq!(stats.video_frames, 0, "nothing is encoded yet");

    let progress: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ExportEvent::Progress(progress) => Some(progress),
            _ => None,
        })
        .collect();
    assert_eq!(
        progress.iter().map(|p| p.frames_done).collect::<Vec<_>>(),
        (1..=frames).collect::<Vec<_>>(),
        "progress is reported once per frame at a zero interval"
    );
    for step in &progress {
        assert_eq!(step.frames_total, frames);
        assert_eq!(step.stats.video_frames, step.frames_done);
        assert_eq!(
            step.stats.encoded.value(),
            i64::try_from(step.frames_done).expect("small")
        );
        assert_eq!(step.stats.encoded.rate(), Rational::FPS_24);
        assert!(
            step.eta.is_some(),
            "an export that knows its length always has an eta"
        );
    }
    let last = progress.last().expect("at least one progress event");
    assert_eq!(last.percent_milli, Some(100_000));
    assert_eq!(last.percent(), Some(100));
    assert_eq!(last.frames_remaining(), Some(0));
    assert_eq!(last.eta, Some(Duration::ZERO), "nothing left to wait for");
    assert!(
        last.frames_per_second_milli > 0,
        "a finished export encoded at some rate"
    );

    match events.last().expect("a terminal event") {
        ExportEvent::Finished(finished) => assert_eq!(finished, &report),
        other => panic!("the last event is Finished, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_export_without_a_total_reports_no_percentage_and_no_eta() {
    let settings = settings();
    let Some(elements) = can_export(&settings) else {
        eprintln!("skipping: this machine has no usable H.264 encoder or matroskamux");
        return;
    };
    let dir = temp_dir("unknown-length");
    let path = dir.join("unknown.mkv");

    let job = ExportJob::new(&path, &settings, &elements).with_progress_interval(Duration::ZERO);
    let mut source = SolidFrames::new(settings.frame_bytes(), 4);
    let mut seen = Vec::new();
    job.run(&mut source, None, &mut |event| {
        if let ExportEvent::Progress(progress) = event {
            seen.push(progress.clone());
        }
    })
    .expect("the export runs");

    assert_eq!(seen.len(), 4);
    for step in &seen {
        assert_eq!(step.frames_total, 0);
        assert_eq!(step.percent_milli, None);
        assert_eq!(step.percent(), None);
        assert_eq!(step.eta, None, "an unknown length cannot be extrapolated");
        assert_eq!(step.frames_remaining(), None);
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancelling_stops_the_pipeline_and_removes_the_partial_file() {
    let settings = settings();
    let Some(elements) = can_export(&settings) else {
        eprintln!("skipping: this machine has no usable H.264 encoder or matroskamux");
        return;
    };
    let dir = temp_dir("cancel");
    let path = dir.join("cancelled.mkv");

    let cancel = CancelToken::new();
    let job = ExportJob::new(&path, &settings, &elements)
        .with_total_frames(400)
        .with_cancel(cancel.clone())
        .with_progress_interval(Duration::ZERO);

    let mut source = CancellingFrames {
        inner: SolidFrames::new(settings.frame_bytes(), 400),
        token: cancel,
        cancel_after: 8,
        handed_out: 0,
    };
    let mut events = Vec::new();
    let error = job
        .run(&mut source, None, &mut |event| events.push(event.clone()))
        .expect_err("a cancelled export does not produce a report");

    assert_eq!(error.code.as_str(), "core.cancelled");
    assert!(
        !path.exists(),
        "a cancelled export leaves no part-written file at {}",
        path.display()
    );

    match events.last().expect("a terminal event") {
        ExportEvent::Cancelled {
            frames_done,
            file_removed,
        } => {
            assert_eq!(
                *frames_done, 9,
                "the eight frames before the cancel and the one already in hand were written"
            );
            assert!(*file_removed, "the part-written file was removed");
        }
        other => panic!("the last event is Cancelled, got {other:?}"),
    }
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ExportEvent::Finished(_))),
        "a cancelled export never reports a finished file"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failing_export_names_the_gstreamer_element_that_broke() {
    let settings = settings();
    let Some(elements) = can_export(&settings) else {
        eprintln!("skipping: this machine has no usable H.264 encoder or matroskamux");
        return;
    };
    let dir = temp_dir("failure");
    // A directory that does not exist: filesink cannot open the file, and the
    // element that says so is the one the error must name.
    let path = dir.join("missing").join("nowhere.mkv");

    let job = ExportJob::new(&path, &settings, &elements).with_total_frames(8);
    let mut source = SolidFrames::new(settings.frame_bytes(), 8);
    let mut events = Vec::new();
    let error = job
        .run(&mut source, None, &mut |event| events.push(event.clone()))
        .expect_err("writing into a missing directory fails");

    let element = error
        .details
        .get("element")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_owned();
    assert!(
        element.contains("filesink"),
        "the error names the failing element, got {:?} in {error}",
        error.details
    );
    assert!(
        error.message.contains(&element),
        "the element name is in the message too: {}",
        error.message
    );
    assert!(
        error.cause.is_some(),
        "the GStreamer error is carried as the cause: {error}"
    );

    match events.last().expect("a terminal event") {
        ExportEvent::Failed { error: failed, .. } => assert_eq!(failed, &error),
        other => panic!("the last event is Failed, got {other:?}"),
    }
    assert!(!path.exists(), "a failed export leaves no file behind");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_spawned_job_reports_progress_to_the_service_and_can_be_cancelled() {
    let settings = settings();
    let Some(elements) = can_export(&settings) else {
        eprintln!("skipping: this machine has no usable H.264 encoder or matroskamux");
        return;
    };
    let dir = temp_dir("spawn");
    let path = dir.join("spawned.mkv");

    let service = JobService::new(1);
    let job = ExportJob::new(&path, &settings, &elements)
        .with_total_frames(16)
        .with_progress_interval(Duration::ZERO);
    let source = Box::new(SolidFrames::new(settings.frame_bytes(), 16));
    let handle = spawn_export_job(&service, job, source, None, Priority::Normal);

    let report = handle.wait().expect("the export runs");
    assert_eq!(report.video_frames, 16);
    assert_eq!(handle.handle().kind(), sub_export::EXPORT_JOB_KIND);
    assert_eq!(handle.handle().wait(), JobOutcome::Completed);

    let events: Vec<_> = handle.events().try_iter().collect();
    let last_progress = events
        .iter()
        .filter_map(|event| match event {
            ExportEvent::Progress(progress) => Some(progress),
            _ => None,
        })
        .next_back()
        .expect("progress reached the export's own stream");
    assert_eq!(last_progress.frames_done, 16);
    assert_eq!(last_progress.percent(), Some(100));
    assert_eq!(last_progress.stats.video_encoder, elements.video_encoder);
    assert_eq!(last_progress.stats.muxer, "matroskamux");
    // The muxer may still be holding its header when the last frame goes in,
    // so the byte count is checked on the finished file rather than mid-run.
    assert!(
        std::fs::metadata(&path).expect("the exported file").len() > 0,
        "the finished export is not empty"
    );

    let _ = std::fs::remove_dir_all(&dir);
    drop(service);
}

#[test]
fn a_spawned_job_cancelled_through_its_handle_removes_its_file() {
    let settings = settings();
    let Some(elements) = can_export(&settings) else {
        eprintln!("skipping: this machine has no usable H.264 encoder or matroskamux");
        return;
    };
    let dir = temp_dir("spawn-cancel");
    let path = dir.join("spawn-cancelled.mkv");

    let service = JobService::new(1);
    let job = ExportJob::new(&path, &settings, &elements)
        .with_total_frames(100_000)
        .with_progress_interval(Duration::ZERO);
    // A long export the test never intends to finish: it is cancelled through
    // the handle, which is the button an operator actually presses.
    let source = Box::new(SolidFrames::new(settings.frame_bytes(), 100_000));
    let handle = spawn_export_job(&service, job, source, None, Priority::Normal);

    wait_for("the export to start writing frames", || {
        handle
            .events()
            .try_iter()
            .any(|event| matches!(event, ExportEvent::Progress(_)))
    });
    handle.cancel();

    let outcome = handle.handle().wait();
    assert_eq!(outcome, JobOutcome::Cancelled, "the job reports the cancel");
    assert_eq!(
        handle.wait().expect_err("no report").code.as_str(),
        "core.cancelled"
    );
    assert!(
        !path.exists(),
        "the cancelled job removed {}",
        path.display()
    );

    let _ = std::fs::remove_dir_all(&dir);
    drop(service);
}

/// Waits for `condition` or fails the test, so no test sleeps a fixed time it
/// does not need.
fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}");
}
