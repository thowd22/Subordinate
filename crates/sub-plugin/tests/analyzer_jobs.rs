//! Analyzer runs are jobs on the shared queue, and what they find lands on the
//! media item as an undoable command (TASK-79).
//!
//! Everything here drives a real [`JobService`] and a real [`Engine`], with a
//! plain Rust analyzer standing in for a WASM instance: the wasmtime store that
//! makes one of those is TASK-84's, and the job around it — priority, progress,
//! cancellation, storing — is what this crate owns.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use sub_core::jobs::{JobEvent, JobService, Priority};
use sub_core::{ErrorCode, SubError};
use sub_edit::{Engine, EngineHandle};
use sub_model::{MediaId, MediaItem, MediaPath, Project};
use sub_plugin::analyzer::JOB_KIND;
use sub_plugin::{
    AnalysisContext, AnalysisJobs, WitAnalysisMarker, WitAnalysisRange, WitAnalysisResult,
    WitDetail, WitRationalTime, WitTimeRange,
};
use sub_time::{Rational, RationalTime, TimeRange};

/// The rate the fixture media runs at: not an integer, on purpose.
fn fps() -> Rational {
    Rational::FPS_23_976
}

/// A WIT time range, the way an analyzer would build one.
fn span(start: i64, duration: i64) -> WitTimeRange {
    WitTimeRange {
        start: WitRationalTime::from(RationalTime::from_frames(start, fps())),
        duration: WitRationalTime::from(RationalTime::from_frames(duration, fps())),
    }
}

/// The same span as the model sees it.
fn model_span(start: i64, duration: i64) -> TimeRange {
    TimeRange::new(
        RationalTime::from_frames(start, fps()),
        RationalTime::from_frames(duration, fps()),
    )
    .unwrap()
}

/// An engine holding one media item, plus the queue analyses run on.
struct Host {
    engine: Engine,
    jobs: Arc<JobService>,
    media: MediaId,
}

impl Host {
    fn new() -> Self {
        let item = MediaItem::new(MediaPath::new("footage/interview.mp4").unwrap());
        let media = item.id;
        let mut project = Project::new("Doc cut");
        project.media.push(item);

        Self {
            engine: Engine::spawn(project).unwrap(),
            jobs: Arc::new(JobService::new(2)),
            media,
        }
    }

    fn handle(&self) -> EngineHandle {
        self.engine.handle().clone()
    }

    fn analyses(&self) -> AnalysisJobs {
        AnalysisJobs::new(Arc::clone(&self.jobs), self.handle())
    }
}

/// What the fixture analyzer reports: one marker, two ranges, one scalar.
fn findings() -> WitAnalysisResult {
    WitAnalysisResult {
        markers: vec![WitAnalysisMarker {
            name: "first frame".to_owned(),
            note: "slate".to_owned(),
            marked_range: span(0, 0),
        }],
        ranges: vec![
            WitAnalysisRange {
                label: "silence".to_owned(),
                range: span(12, 6),
            },
            WitAnalysisRange {
                label: "speech".to_owned(),
                range: span(18, 30),
            },
        ],
        metadata: vec![WitDetail {
            key: "lufs".to_owned(),
            value: "-23.5".to_owned(),
        }],
    }
}

/// Blocks until `condition` holds, failing the test rather than hanging.
fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}");
}

/// The progress reported by analysis jobs, in order.
fn progress_of(events: &Receiver<JobEvent>) -> Vec<(u64, u64)> {
    events
        .try_iter()
        .filter_map(|event| match event {
            JobEvent::Progress {
                kind, done, total, ..
            } if kind == JOB_KIND => Some((done, total)),
            _ => None,
        })
        .collect()
}

#[test]
fn findings_land_on_the_media_item_and_can_be_undone() {
    let host = Host::new();
    let events = host.jobs.subscribe();
    let media = host.media;

    let handle = host.analyses().submit_background(
        "silence",
        media,
        "{\"threshold_db\":-40}",
        move |asked: MediaId, options: &str, ctx: &AnalysisContext<'_>| {
            assert_eq!(asked, media, "the job hands the analyzer its media item");
            assert!(options.contains("threshold_db"), "options cross verbatim");
            ctx.report_progress(1, 2);
            ctx.report_progress(2, 2);
            Ok(findings())
        },
    );

    assert_eq!(handle.kind(), JOB_KIND);
    assert_eq!(handle.priority(), Priority::Background);
    let outcome = handle.wait();
    assert!(outcome.is_completed(), "outcome was {outcome:?}");

    let project = host.engine.handle().snapshot();
    let item = project.media_item(media).unwrap();
    let analysis = item.analysis("silence").expect("findings were stored");
    assert_eq!(analysis.markers.len(), 1);
    assert_eq!(analysis.markers[0].name, "first frame");
    assert_eq!(analysis.markers[0].note, "slate");
    assert_eq!(analysis.ranges.len(), 2);
    assert_eq!(analysis.ranges[0].label, "silence");
    assert_eq!(analysis.ranges[0].range, model_span(12, 6));
    assert_eq!(
        analysis.metadata.get("lufs"),
        Some(&serde_json::json!(-23.5))
    );

    let progress = progress_of(&events);
    assert_eq!(progress, [(1, 2), (2, 2)], "progress reached the queue");

    // Storing went through the engine as a command, so it undoes.
    host.engine.handle().undo().unwrap().unwrap();
    let project = host.engine.handle().snapshot();
    assert!(project.media_item(media).unwrap().analyses.is_empty());

    host.engine.shutdown().unwrap();
}

#[test]
fn a_second_run_of_the_same_analyzer_replaces_the_first() {
    let host = Host::new();
    let media = host.media;
    let analyses = host.analyses();

    assert!(
        analyses
            .submit_background(
                "silence",
                media,
                "{}",
                move |_: MediaId, _: &str, _: &AnalysisContext<'_>| { Ok(findings()) }
            )
            .wait()
            .is_completed()
    );
    let loudness = analyses
        .submit_background(
            "loudness",
            media,
            "{}",
            move |_: MediaId, _: &str, _: &AnalysisContext<'_>| {
                Ok(WitAnalysisResult {
                    markers: Vec::new(),
                    ranges: Vec::new(),
                    metadata: vec![WitDetail {
                        key: "lufs".to_owned(),
                        value: "-18".to_owned(),
                    }],
                })
            },
        )
        .wait();
    assert!(loudness.is_completed());
    let rerun = analyses
        .submit_background(
            "silence",
            media,
            "{}",
            move |_: MediaId, _: &str, _: &AnalysisContext<'_>| {
                Ok(WitAnalysisResult {
                    markers: Vec::new(),
                    ranges: vec![WitAnalysisRange {
                        label: "silence".to_owned(),
                        range: span(0, 4),
                    }],
                    metadata: Vec::new(),
                })
            },
        )
        .wait();
    assert!(rerun.is_completed());

    let project = host.engine.handle().snapshot();
    let item = project.media_item(media).unwrap();
    assert_eq!(item.analyses.len(), 2, "one entry per analyzer");
    assert_eq!(item.analysis("silence").unwrap().ranges.len(), 1);
    assert!(item.analysis("loudness").is_some());

    host.engine.shutdown().unwrap();
}

#[test]
fn cancelling_a_running_analysis_stores_nothing() {
    let host = Host::new();
    let media = host.media;
    let (started, has_started) = channel();
    let saw_cancel = Arc::new(AtomicBool::new(false));
    let reports_cancel = Arc::clone(&saw_cancel);

    let handle = host.analyses().submit_background(
        "transcript",
        media,
        "{}",
        move |_: MediaId, _: &str, ctx: &AnalysisContext<'_>| {
            started.send(()).unwrap();
            // A long analyzer asks between units of work, which is the only
            // way cancellation reaches code inside the sandbox.
            for unit in 0..10_000_u64 {
                if ctx.is_cancelled() {
                    reports_cancel.store(true, Ordering::Release);
                    return Err(SubError::new(
                        ErrorCode::from_static("core.cancelled"),
                        "analysis was cancelled",
                    ));
                }
                ctx.report_progress(unit, 10_000);
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(findings())
        },
    );

    has_started.recv_timeout(Duration::from_secs(5)).unwrap();
    handle.cancel();
    let outcome = handle.wait();
    assert!(!outcome.is_completed(), "outcome was {outcome:?}");
    assert!(saw_cancel.load(Ordering::Acquire), "the analyzer was asked");

    let project = host.engine.handle().snapshot();
    assert!(project.media_item(media).unwrap().analyses.is_empty());

    host.engine.shutdown().unwrap();
}

#[test]
fn a_run_cancelled_after_it_finished_still_stores_nothing() {
    let host = Host::new();
    let media = host.media;
    let handle = host.analyses().submit_background(
        "silence",
        media,
        "{}",
        move |_: MediaId, _: &str, ctx: &AnalysisContext<'_>| {
            // The analyzer ignored the flag and returned findings anyway; the
            // host is what refuses to store them.
            while !ctx.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(findings())
        },
    );

    wait_for("the analysis to start", || host.jobs.running_len() == 1);
    handle.cancel();
    let outcome = handle.wait();
    assert!(outcome.is_cancelled(), "outcome was {outcome:?}");

    let project = host.engine.handle().snapshot();
    assert!(project.media_item(media).unwrap().analyses.is_empty());

    host.engine.shutdown().unwrap();
}

#[test]
fn a_queued_analysis_that_is_cancelled_never_runs() {
    let host = Host::new();
    let media = host.media;
    let analyses = host.analyses();
    let (release, blocked) = channel::<()>();
    let mut blockers = Vec::new();

    // Fill both workers so the analysis stays queued.
    for _ in 0..2 {
        let release = release.clone();
        blockers.push(
            host.jobs
                .submit("block", Priority::Interactive, move |ctx| {
                    release.send(()).unwrap();
                    while !ctx.is_cancelled() {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Ok(())
                }),
        );
    }
    for _ in 0..2 {
        blocked.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    let ran = Arc::new(AtomicBool::new(false));
    let reports_run = Arc::clone(&ran);
    let handle = analyses.submit_background(
        "silence",
        media,
        "{}",
        move |_: MediaId, _: &str, _: &AnalysisContext<'_>| {
            reports_run.store(true, Ordering::Release);
            Ok(findings())
        },
    );
    handle.cancel();
    for blocker in &blockers {
        blocker.cancel();
    }

    assert!(handle.wait().is_cancelled());
    assert!(!ran.load(Ordering::Acquire), "the analyzer never ran");
    let project = host.engine.handle().snapshot();
    assert!(project.media_item(media).unwrap().analyses.is_empty());

    host.engine.shutdown().unwrap();
}

#[test]
fn a_failing_analyzer_fails_its_job_and_stores_nothing() {
    let host = Host::new();
    let media = host.media;
    let handle = host.analyses().submit_background(
        "silence",
        media,
        "{}",
        move |_: MediaId, _: &str, _: &AnalysisContext<'_>| {
            Err(SubError::new(
                ErrorCode::from_static("plugin.analysis_failed"),
                "the decoder gave up",
            ))
        },
    );

    let outcome = handle.wait();
    let error = outcome.error().expect("the job failed");
    assert_eq!(error.code.as_str(), "plugin.analysis_failed");

    let project = host.engine.handle().snapshot();
    assert!(project.media_item(media).unwrap().analyses.is_empty());

    host.engine.shutdown().unwrap();
}

#[test]
fn findings_a_plugin_could_not_have_meant_are_refused() {
    let host = Host::new();
    let media = host.media;

    let bad_time = host
        .analyses()
        .submit_background(
            "silence",
            media,
            "{}",
            move |_: MediaId, _: &str, _: &AnalysisContext<'_>| {
                Ok(WitAnalysisResult {
                    markers: Vec::new(),
                    ranges: vec![WitAnalysisRange {
                        label: "silence".to_owned(),
                        range: WitTimeRange {
                            start: WitRationalTime::from(RationalTime::from_frames(0, fps())),
                            duration: WitRationalTime::from(RationalTime::from_frames(-4, fps())),
                        },
                    }],
                    metadata: Vec::new(),
                })
            },
        )
        .wait();
    assert_eq!(
        bad_time.error().map(|err| err.code.as_str()),
        Some("plugin.invalid_time_range")
    );

    let bad_metadata = host
        .analyses()
        .submit_background(
            "silence",
            media,
            "{}",
            move |_: MediaId, _: &str, _: &AnalysisContext<'_>| {
                Ok(WitAnalysisResult {
                    markers: Vec::new(),
                    ranges: Vec::new(),
                    metadata: vec![WitDetail {
                        key: "lufs".to_owned(),
                        value: "not json".to_owned(),
                    }],
                })
            },
        )
        .wait();
    assert_eq!(
        bad_metadata.error().map(|err| err.code.as_str()),
        Some("plugin.invalid_metadata")
    );

    let project = host.engine.handle().snapshot();
    assert!(project.media_item(media).unwrap().analyses.is_empty());

    host.engine.shutdown().unwrap();
}

#[test]
fn an_analysis_of_media_the_project_does_not_hold_fails() {
    let host = Host::new();
    let outcome = host
        .analyses()
        .submit_background(
            "silence",
            MediaId::new(),
            "{}",
            move |_: MediaId, _: &str, _: &AnalysisContext<'_>| Ok(findings()),
        )
        .wait();

    assert_eq!(
        outcome.error().map(|err| err.code.as_str()),
        Some("edit.media_not_found")
    );

    host.engine.shutdown().unwrap();
}
