//! The assembled window exports through its socket and its own panel.
mod support;

use std::time::{Duration, Instant};

use egui_kittest::Harness;
use serde_json::json;
use sub_command::transport::Client;
use sub_model::sequence::SequenceSettings;
use sub_model::{Project, Sequence};
use sub_ui::export_panel::ExportStatus;
use sub_ui::{AppOptions, SubordinateApp};

#[test]
#[allow(clippy::too_many_lines)]
fn socket_exports_appear_in_the_panel_and_keep_the_window_painting() {
    if !support::can_render() {
        return;
    }
    sub_media::init().expect("GStreamer initializes");
    if !sub_export::element_is_usable("x264enc") || !sub_export::element_is_usable("matroskamux") {
        eprintln!("skipping: x264enc and matroskamux are required");
        return;
    }
    let dir = std::env::temp_dir().join(format!("sub-ui-host-{}", sub_model::ProjectId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("project.sub");
    let mut project = Project::new("Socket export");
    project
        .sequences
        .push(Sequence::new("Main", SequenceSettings::default()));
    std::fs::write(&path, sub_model::json::to_json(&project).unwrap()).unwrap();
    let options = AppOptions {
        project: Some(path),
        serve_command_api: true,
        instance: Some("host-test".to_owned()),
        endpoint_dir: Some(dir.join("endpoint")),
        ..AppOptions::default()
    };
    let mut harness: Harness<'_, SubordinateApp> = support::builder()
        .with_size(eframe::egui::vec2(1400.0, 900.0))
        .build_eframe(move |cc| SubordinateApp::new(cc, options).unwrap());
    let deadline = Instant::now() + Duration::from_secs(20);
    let endpoint = loop {
        harness.step();
        let api = harness.state().command_api().unwrap();
        assert!(api.refusal().is_none(), "{:?}", api.refusal());
        if api.is_serving() {
            break api.endpoint().clone();
        }
        assert!(Instant::now() < deadline, "endpoint did not bind");
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut client = Client::connect(&endpoint).unwrap();
    let methods = client
        .invoke("system.list_methods", None)
        .unwrap()
        .to_string();
    for method in [
        "export.render",
        "export.progress",
        "export.list_presets",
        "media.probe",
        "media.make_proxy",
        "playback.render_frame_png",
    ] {
        assert!(methods.contains(method), "missing {method}: {methods}");
    }
    let presets = client.invoke("export.list_presets", None).unwrap();
    assert!(
        presets["presets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|preset| preset["id"] == "mezzanine")
    );
    let frame = client
        .invoke(
            "playback.render_frame_png",
            Some(json!({
                "time": sub_time::RationalTime::from_frames(0, sub_time::Rational::FPS_30),
                "width": 64
            })),
        )
        .unwrap();
    assert_eq!(frame["mime_type"], "image/png");
    assert_eq!(frame["width"], 64);
    assert!(frame["data"].as_str().unwrap().starts_with("iVBORw0KGgo"));
    let missing_proxy = client
        .invoke(
            "media.make_proxy",
            Some(json!({
                "media": sub_model::MediaId::new()
            })),
        )
        .unwrap_err();
    assert_eq!(missing_proxy.code, sub_core::codes::NOT_FOUND);
    harness
        .state_mut()
        .export_panel()
        .set_encoder_override(Some("x264enc"))
        .unwrap();

    // The second job catches accidental publication of the first job's
    // terminal panel state before the second worker posts its Started event.
    for number in 1..=2 {
        let output = dir.join(format!("export-{number}.mkv"));
        let started = client
            .invoke(
                "export.render",
                Some(json!({
                    "preset": "mezzanine", "output": output,
                    "range": {"start_frame": 0, "end_frame": 3}
                })),
            )
            .unwrap();
        assert_eq!(started["state"], "running");
        let job = started["job"].as_str().unwrap();
        let busy = client
            .invoke(
                "export.render",
                Some(json!({
                    "preset": "mezzanine", "output": dir.join("busy.mkv")
                })),
            )
            .unwrap_err();
        assert_eq!(busy.code, sub_ui::codes::EXPORT_BUSY);
        if number == 1 {
            // The accepted job keeps its snapshot even when the next socket
            // command replaces the editor project before the UI pumps it.
            client
                .invoke("project.new", Some(json!({"name": "Other project"})))
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_mins(1);
        let mut painted = 0;
        loop {
            harness.step();
            painted += 1;
            assert_eq!(harness.state_mut().export_panel().output(), output);
            let status = client
                .invoke("export.progress", Some(json!({"job": job})))
                .unwrap();
            if status["state"] == "completed" {
                let ExportStatus::Finished(report) = harness.state().export_status() else {
                    panic!("API completion disagrees with panel");
                };
                assert_eq!(report.path, output);
                assert!(report.video_frames > 0);
                assert_eq!(status["frames_done"], report.video_frames);
                assert!(output.is_file());
                break;
            }
            assert_eq!(status["state"], "running", "{status}");
            assert!(Instant::now() < deadline, "export timed out: {status}");
        }
        assert!(painted > 1, "the window paints while its export runs");
        let probed = client
            .invoke("media.probe", Some(json!({"path": output})))
            .unwrap();
        assert!(probed.is_object());
        if number == 1 {
            let other_dir = dir.join("other");
            std::fs::create_dir_all(&other_dir).unwrap();
            std::fs::copy(&output, other_dir.join("only-here.mkv")).unwrap();
            let mut other = Project::new("Another saved project");
            other
                .sequences
                .push(Sequence::new("Other", SequenceSettings::default()));
            let media =
                sub_model::MediaItem::new(sub_model::MediaPath::new("only-here.mkv").unwrap());
            let media_id = media.id;
            other.media.push(media);
            let other_path = other_dir.join("project.sub");
            std::fs::write(&other_path, sub_model::json::to_json(&other).unwrap()).unwrap();
            client
                .invoke("project.open", Some(json!({"path": other_path})))
                .unwrap();
            // No UI frame between open and probe: the socket updates context
            // synchronously, and this file does not exist in the old folder.
            let probe = client
                .invoke("media.probe", Some(json!({"media": media_id})))
                .unwrap();
            assert_eq!(
                probe["path"],
                other_dir.join("only-here.mkv").display().to_string()
            );
        }
    }
    drop(client);
    // Opening another project rebuilds the engine dispatcher. Host methods
    // must survive that replacement as well as the initial startup.
    harness
        .state_mut()
        .session_mut()
        .adopt(project, Some(dir.join("project.sub")))
        .unwrap();
    let rebuilt = harness
        .state()
        .session()
        .commands_arc()
        .invoke("system.list_methods", None)
        .unwrap()
        .to_string();
    assert_eq!(rebuilt, methods);
    drop(harness);
    std::fs::remove_dir_all(dir).unwrap();
}
