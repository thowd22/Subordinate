//! A deterministic slow encoder regression, independent of AV1 speed or hardware.
use super::*;

fn delayed_pipeline(name: &str, limit: Option<u64>) -> Option<ExportPipeline> {
    gst::init().expect("GStreamer initialises");
    if !element_is_usable("x264enc") || !element_is_usable("matroskamux") {
        eprintln!("skipping: x264enc or matroskamux is unavailable");
        return None;
    }
    let settings = ExportSettings::new(16, 16, Rational::FPS_24, Container::Mkv)
        .with_audio_codec(None)
        .with_stall_timeout_ms(300)
        .with_timeout_ms(limit);
    let elements = ExportElements {
        video_encoder: "x264enc".to_owned(),
        audio_encoder: None,
    };
    let path = std::env::temp_dir().join(format!("sub-export-{name}-{}.mkv", std::process::id()));
    let mut pipeline = ExportPipeline::new(&path, &settings, &elements).expect("pipeline builds");
    // Delay each buffer on the streaming thread, downstream of appsrc. The
    // caller queues all frames immediately; finish must observe real draining.
    pipeline
        .video_src
        .static_pad("src")
        .unwrap()
        .add_probe(gst::PadProbeType::BUFFER, |_, _| {
            std::thread::sleep(Duration::from_millis(50));
            gst::PadProbeReturn::Ok
        });
    for _ in 0..24 {
        pipeline
            .push_video_frame(&vec![255; settings.frame_bytes()])
            .expect("frame queues");
    }
    Some(pipeline)
}

#[test]
fn a_slow_encoder_finishes_an_export_that_a_fixed_budget_would_abandon() {
    let Some(pipeline) = delayed_pipeline("slow", None) else {
        return;
    };
    let path = pipeline.path().to_owned();
    let started = Instant::now();
    let report = pipeline
        .finish()
        .expect("progress keeps resetting patience");
    assert_eq!(report.video_frames, 24);
    assert!(started.elapsed() > Duration::from_millis(300));
    assert!(std::fs::metadata(&path).unwrap().len() > 0);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_hard_limit_in_the_request_fails_even_while_progress_continues() {
    let Some(pipeline) = delayed_pipeline("limit", Some(400)) else {
        return;
    };
    let path = pipeline.path().to_owned();
    let error = pipeline
        .finish()
        .expect_err("progress cannot override an explicit hard limit");
    assert_eq!(error.code, codes::EXPORT_TIMEOUT);
    assert_eq!(error.details["reason"], "time_limit");
    assert_eq!(error.details["element"], "x264enc");
    assert!(error.message.contains("x264enc"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_drained_appsrc_still_names_the_encoder_until_it_emits_eos() {
    gst::init().unwrap();
    if !element_is_usable("x264enc") || !element_is_usable("matroskamux") {
        eprintln!("skipping: x264enc or matroskamux is unavailable");
        return;
    }
    let settings = ExportSettings::new(16, 16, Rational::FPS_24, Container::Mkv)
        .with_audio_codec(None)
        .with_stall_timeout_ms(100);
    let elements = ExportElements {
        video_encoder: "x264enc".to_owned(),
        audio_encoder: None,
    };
    let path = std::env::temp_dir().join(format!("sub-export-flush-{}.mkv", std::process::id()));
    let mut pipeline = ExportPipeline::new(&path, &settings, &elements).unwrap();
    let encoder = pipeline
        .pipeline
        .iterate_elements()
        .into_iter()
        .filter_map(Result::ok)
        .find(|element| {
            element
                .factory()
                .is_some_and(|factory| factory.name() == "x264enc")
        })
        .unwrap();
    encoder.static_pad("sink").unwrap().add_probe(
        gst::PadProbeType::EVENT_DOWNSTREAM,
        |_, info| {
            if info
                .event()
                .is_some_and(|event| event.type_() == gst::EventType::Eos)
            {
                std::thread::sleep(Duration::from_millis(500));
            }
            gst::PadProbeReturn::Ok
        },
    );
    pipeline
        .push_video_frame(&vec![255; settings.frame_bytes()])
        .unwrap();
    let error = pipeline
        .finish()
        .expect_err("an encoder holding EOS has stalled");
    assert_eq!(error.code, codes::EXPORT_TIMEOUT);
    assert_eq!(error.details["reason"], "stalled");
    assert_eq!(error.details["element"], "x264enc");
    assert!(error.message.contains("x264enc"));
    let _ = std::fs::remove_file(path);
}
