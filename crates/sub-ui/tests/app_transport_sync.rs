//! The assembled editor keeps its playhead, timecode and timeline in sync.
mod support;

use eframe::egui;
use egui_kittest::kittest::Queryable;
use sub_model::{Gap, Project, Sequence, SequenceSettings, Track, TrackItem, TrackKind};
use sub_time::{Rational, RationalTime};
use sub_ui::timeline::ZoomLevel;
use sub_ui::{AppOptions, SubordinateApp};

fn project() -> Project {
    let mut project = Project::new("Transport");
    let mut sequence = Sequence::new("Main", SequenceSettings::default());
    sequence.settings.frame_rate = Rational::FPS_24;
    let mut track = Track::new("Video", TrackKind::Video);
    track.items.push(TrackItem::Gap(Gap::new(RationalTime::new(
        2400,
        Rational::FPS_24,
    ))));
    sequence.tracks.push(track);
    project.sequences.push(sequence);
    project
}

#[test]
fn dragging_the_visible_playhead_updates_timecode_without_editing_the_timeline() {
    if !support::can_render() {
        return;
    }
    let mut harness = support::builder::<SubordinateApp>()
        .with_size(egui::vec2(1400.0, 900.0))
        .build_eframe(|cc| {
            SubordinateApp::new(
                cc,
                AppOptions {
                    open_audio_output: false,
                    ..AppOptions::default()
                },
            )
            .unwrap()
        });
    let original = project();
    harness.state_mut().adopt_project(original.clone()).unwrap();
    harness
        .state_mut()
        .timeline()
        .view_mut()
        .set_zoom(ZoomLevel::ONE);
    harness.state_mut().viewer().state.seek_to_frame(48);
    support::run_settled(&mut harness);
    let layout = harness.state_mut().timeline().layout().unwrap();
    let from = layout.content.left_top() + egui::vec2(48.0, 24.0);
    let to = from + egui::vec2(72.0, 0.0);
    harness.hover_at(from);
    support::run_settled(&mut harness);
    harness.drag_at(from);
    support::run_settled(&mut harness);
    harness.hover_at(to);
    support::run_settled(&mut harness);
    harness.drop_at(to);
    support::run_settled(&mut harness);
    assert_eq!(harness.state_mut().viewer().state.playhead_frame(), 120);
    assert_eq!(harness.state_mut().timeline().playhead().value(), 120);
    harness.get_by_label("00:00:05:00");
    assert_eq!(*harness.state().session().project_arc(), original);
}

#[test]
fn assembled_fallback_playback_advances_at_one_x_and_updates_both_panels() {
    if !support::can_render() {
        return;
    }
    let mut harness = support::builder::<SubordinateApp>()
        .with_size(egui::vec2(1400.0, 900.0))
        .with_step_dt(1.0 / 60.0)
        .build_eframe(|cc| {
            SubordinateApp::new(
                cc,
                AppOptions {
                    open_audio_output: false,
                    ..AppOptions::default()
                },
            )
            .unwrap()
        });
    harness.state_mut().adopt_project(project()).unwrap();
    support::run_settled(&mut harness);
    harness.key_press(egui::Key::Space);
    harness.step();
    let before = harness.state_mut().viewer().state.playhead_frame();
    for _ in 0..60 {
        harness.step();
    }
    let after = harness.state_mut().viewer().state.playhead_frame();
    assert!(
        (23..=25).contains(&(after - before)),
        "one second at 24 fps advanced {} frames",
        after - before
    );
    assert_eq!(harness.state_mut().timeline().playhead().value(), after);
    let label = harness.state_mut().viewer().state.timecode_label();
    harness.get_by_label(&label);
    harness.key_press(egui::Key::Space);
    harness.step();
}

#[test]
fn default_audio_playback_moves_the_clock_and_timecode_after_scrubbing() {
    if !support::can_render() {
        return;
    }
    let mut harness = support::builder::<SubordinateApp>()
        .with_size(egui::vec2(1400.0, 900.0))
        .build_eframe(|cc| SubordinateApp::new(cc, AppOptions::default()).unwrap());
    harness.state_mut().adopt_project(project()).unwrap();
    support::run_settled(&mut harness);
    // Exercise the transition from a paused scrub grain to ordinary playback.
    let layout = harness.state_mut().timeline().layout().unwrap();
    let pos = layout.ruler.left_top() + egui::vec2(72.0, 8.0);
    harness.event(egui::Event::PointerMoved(pos));
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        });
        harness.step();
    }
    let before = harness.state_mut().viewer().state.playhead_frame();
    harness.key_press(egui::Key::Space);
    harness.step();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        harness.step();
        if harness.state_mut().viewer().state.playhead_frame() > before + 2 {
            break;
        }
    }
    let after = harness.state_mut().viewer().state.playhead_frame();
    assert!(
        after > before + 2,
        "normal playback remained parked after a scrub: {before} -> {after}"
    );
    assert_eq!(harness.state_mut().timeline().playhead().value(), after);
    let label = harness.state_mut().viewer().state.timecode_label();
    harness.get_by_label(&label);
    harness.key_press(egui::Key::Space);
    harness.step();
}

struct McpTransport {
    child: std::process::Child,
    input: std::process::ChildStdin,
    output: std::io::BufReader<std::process::ChildStdout>,
    id: u64,
}

impl McpTransport {
    fn start(binary: &std::path::Path, directory: &std::path::Path) -> Self {
        use std::io::Write;
        let mut child = std::process::Command::new(binary)
            .env("SUBORDINATE_ENDPOINT_DIR", directory)
            .env("SUBORDINATE_INSTANCE", "gui-transport")
            .env("SUBORDINATE_MCP_NO_LAUNCH", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .expect("the actual MCP bridge starts");
        let input = child.stdin.take().unwrap();
        let output = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut client = Self {
            child,
            input,
            output,
            id: 0,
        };
        client.request(
            "initialize",
            &serde_json::json!({
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "gui-transport-test", "version": "1"}
            }),
        );
        writeln!(
            client.input,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0", "method": "notifications/initialized"
            })
        )
        .unwrap();
        client.input.flush().unwrap();
        client
    }

    fn request(&mut self, method: &str, params: &serde_json::Value) -> serde_json::Value {
        use std::io::{BufRead, Write};
        self.id += 1;
        writeln!(
            self.input,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0", "id": self.id, "method": method, "params": params
            })
        )
        .unwrap();
        self.input.flush().unwrap();
        loop {
            let mut line = String::new();
            assert!(
                self.output.read_line(&mut line).unwrap() > 0,
                "MCP closed stdout"
            );
            let response: serde_json::Value = serde_json::from_str(&line).unwrap();
            if response["id"] == self.id {
                assert!(response.get("error").is_none(), "{response}");
                return response["result"].clone();
            }
        }
    }
}

impl Drop for McpTransport {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

enum RemoteTransport {
    Socket(sub_command::Client),
    Mcp(McpTransport),
}

impl RemoteTransport {
    fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        match self {
            Self::Socket(client) => client.invoke(method, Some(params)).unwrap(),
            Self::Mcp(client) => {
                let result = client.request(
                    "tools/call",
                    &serde_json::json!({
                        "name": method.replace('.', "_"), "arguments": params
                    }),
                );
                assert_ne!(result["isError"], serde_json::json!(true), "{result}");
                result["structuredContent"].clone()
            }
        }
    }
}

fn mcp_binary() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("SUBORDINATE_MCP") {
        let path = std::path::PathBuf::from(path);
        assert!(
            path.is_file(),
            "SUBORDINATE_MCP does not name a binary: {}",
            path.display()
        );
        return Some(path);
    }
    let current = std::env::current_exe().unwrap();
    let directory = current.parent().unwrap();
    let name = format!("subordinate-mcp{}", std::env::consts::EXE_SUFFIX);
    [
        directory.join(&name),
        directory.parent().unwrap().join(&name),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

#[test]
fn remote_transport_controls_the_assembled_gui_and_reports_its_actual_playhead() {
    if !support::can_render() {
        return;
    }
    let binary = mcp_binary();
    if binary.is_none() {
        eprintln!("skipping: actual MCP bridge not built; socket GUI transport still runs");
    }
    for use_mcp in [false, true] {
        if use_mcp && binary.is_none() {
            continue;
        }
        let mut harness = support::builder::<SubordinateApp>()
            .with_size(egui::vec2(1400.0, 900.0))
            .with_step_dt(1.0 / 60.0)
            .build_eframe(|cc| {
                SubordinateApp::new(
                    cc,
                    AppOptions {
                        open_audio_output: false,
                        serve_command_api: false,
                        ..AppOptions::default()
                    },
                )
                .unwrap()
            });
        harness.state_mut().adopt_project(project()).unwrap();
        support::run_settled(&mut harness);
        #[cfg(unix)]
        let base = std::path::PathBuf::from("/tmp");
        #[cfg(not(unix))]
        let base = std::env::temp_dir();
        let directory = base.join(format!(
            "sub-transport-{}-{}",
            std::process::id(),
            u8::from(use_mcp)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let endpoint =
            sub_command::Endpoint::in_directory(directory.clone(), "gui-transport").unwrap();
        let server =
            sub_command::Server::bind(endpoint.clone(), harness.state().session().commands_arc())
                .unwrap();
        let mut remote = if use_mcp {
            RemoteTransport::Mcp(McpTransport::start(binary.as_ref().unwrap(), &directory))
        } else {
            RemoteTransport::Socket(sub_command::Client::connect(&endpoint).unwrap())
        };
        let position = RationalTime::new(96, Rational::FPS_24);
        remote.call("playback.seek", serde_json::json!({"position": position}));
        harness.step();
        assert_eq!(harness.state_mut().viewer().state.playhead(), position);
        assert_eq!(harness.state_mut().timeline().playhead(), position);
        harness.get_by_label("00:00:04:00");
        remote.call("playback.play", serde_json::json!({}));
        harness.step();
        let before = harness.state_mut().viewer().state.playhead_frame();
        for _ in 0..60 {
            harness.step();
        }
        let after = harness.state_mut().viewer().state.playhead_frame();
        assert!(
            (23..=25).contains(&(after - before)),
            "GUI did not play at 1x: {before} -> {after}"
        );
        let status = remote.call("playback.status", serde_json::json!({}));
        assert_eq!(status["playing"], true);
        assert_eq!(
            status["position"],
            serde_json::json!(harness.state_mut().viewer().state.playhead())
        );
        remote.call("playback.pause", serde_json::json!({}));
        harness.step();
        let paused = harness.state_mut().viewer().state.playhead_frame();
        for _ in 0..5 {
            harness.step();
        }
        assert_eq!(harness.state_mut().viewer().state.playhead_frame(), paused);
        harness.key_press(egui::Key::Home);
        harness.step();
        let status = remote.call("playback.status", serde_json::json!({}));
        assert_eq!(status["playing"], false);
        assert_eq!(
            status["position"],
            serde_json::json!(RationalTime::zero(Rational::FPS_24))
        );
        drop(remote);
        server.shutdown().unwrap();
        drop(harness);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
