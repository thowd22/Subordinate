//! The export panel on the shared `egui_kittest` harness.
//!
//! One snapshot holds the panel's picture: the preset picker with a plugin's
//! preset in the same list as the built-ins, the range, the output file, the
//! encoder override, a finished export's status line and the recent list.
//!
//! The interaction test drives the panel the way a user does — it clicks "In
//! to out" by the label in the accessible tree, then clicks Export — and
//! asserts on the [`ExportRequest`] the panel handed back, because that
//! request, and not a rectangle, is what an export is actually made of.
//!
//! Neither test renders anything itself: the panel's frames come from the
//! committed sample project, so the snapshot moves only when the panel or the
//! fixture does.

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use eframe::egui;
use egui_kittest::kittest::Queryable;
use sub_export::{ExportEvent, ExportReport, PresetLibrary};
use sub_model::Project;
use sub_plugin::ExportPreset;
use sub_plugin::manifest::PluginId;
use sub_time::{Rational, RationalTime};
use sub_ui::export_panel::{ExportAction, ExportPanel, ExportRange, ExportRequest};

/// The plugin whose presets join the list, as a manifest would name it.
fn plugin() -> PluginId {
    PluginId::parse("com.example.exporter").expect("a valid plugin id")
}

/// One preset that plugin offers.
fn plugin_preset() -> ExportPreset {
    ExportPreset {
        id: "prores.proxy".to_owned(),
        name: "ProRes Proxy".to_owned(),
        description: "editorial hand-off".to_owned(),
        container: "mov".to_owned(),
        frame_rate: Some(Rational::FPS_24),
        settings: BTreeMap::new(),
    }
}

/// The panel over the sample project, with the built-in and plugin presets
/// loaded and a fixed output file so nothing in the picture varies by machine.
fn scene() -> (ExportPanel, Project) {
    let project = support::fixture_project();
    let sequence = support::fixture_sequence(&project).id;
    let mut panel = ExportPanel::new();
    panel.set_library(&PresetLibrary::builtin());
    panel.set_plugin_presets(&plugin(), &[plugin_preset()]);
    panel.select_sequence(&project, sequence);
    panel.set_output("/renders/sample.mp4");
    (panel, project)
}

#[test]
fn the_export_panel_matches_its_snapshot() {
    if !support::can_render() {
        return;
    }
    let (mut panel, project) = scene();
    // A finished export, so the status line and the recent list are both in
    // the picture. The report is written by hand rather than encoded: this is
    // a test of the panel, not of GStreamer.
    panel.apply_event(&ExportEvent::Finished(ExportReport {
        path: PathBuf::from("/renders/sample.mp4"),
        video_frames: 240,
        audio_frames: 480,
        duration: RationalTime::from_frames(240, Rational::FPS_24),
        video_encoder: "x264enc".to_owned(),
        audio_encoder: Some("avenc_aac".to_owned()),
        muxer: "mp4mux".to_owned(),
    }));

    let mut harness = support::panel_harness(|ui| {
        panel.ui(ui, &project);
    });
    harness.run();
    support::snapshot(&mut harness, "export_panel_settings");
}

/// The panel, the project it exports and whatever the last click asked for.
struct Scene {
    panel: ExportPanel,
    project: Project,
    asked: Option<ExportAction>,
}

#[test]
fn changing_the_range_changes_the_request_the_export_button_builds() {
    let (mut panel, project) = scene();
    // An in and out that are not the whole sequence, so the click on "In to
    // out" changes the request rather than describing the same frames twice.
    panel.set_in_out(24, 72);
    let whole = panel
        .request(&project)
        .expect("the panel is ready to export")
        .frames_total();

    let mut harness = support::panel_harness_state(
        Scene {
            panel,
            project,
            asked: None,
        },
        |ui, scene| {
            if let Some(action) = scene.panel.ui(ui, &scene.project) {
                scene.asked = Some(action);
            }
        },
    );
    harness.run();
    assert_eq!(
        harness.state().panel.range(),
        ExportRange::WholeSequence,
        "the panel starts on the whole sequence"
    );

    harness.get_by_label(ExportRange::InToOut.label()).click();
    harness.run();
    assert_eq!(harness.state().panel.range(), ExportRange::InToOut);

    // By role as well as label: the panel's heading reads "Export" too, and
    // it is the button this test means.
    harness
        .get_by_role_and_label(
            egui::accesskit::Role::Button,
            sub_ui::export_panel::START_LABEL,
        )
        .click();
    harness.run();

    let Some(ExportAction::Start(request)) = harness.state().asked.clone() else {
        panic!(
            "clicking Export asks for an export: {:?}",
            harness.state().asked
        );
    };
    let request: ExportRequest = *request;
    assert_eq!(request.range, ExportRange::InToOut);
    assert_eq!(
        request.frames_total(),
        48,
        "the request covers exactly the frames between the points"
    );
    assert_ne!(request.frames_total(), whole, "and not the whole sequence");
    assert_eq!(
        request.span.start(),
        RationalTime::from_frames(24, request.span.start().rate()),
        "starting on the in point, in exact frames"
    );
    assert_eq!(request.output, PathBuf::from("/renders/sample.mp4"));
    assert_eq!(
        request.sequence,
        support::fixture_sequence(&harness.state().project).id
    );
    assert_eq!(
        request.preset.id,
        harness
            .state()
            .panel
            .presets()
            .first()
            .expect("the library is loaded")
            .id,
        "the first preset is chosen until the user picks another"
    );
}

#[test]
fn a_plugins_preset_is_offered_beside_the_built_in_ones() {
    let (panel, project) = scene();
    let entry = panel
        .presets()
        .iter()
        .find(|entry| entry.id == "prores.proxy")
        .expect("the exporter plugin's preset is listed");
    assert_eq!(entry.name, "ProRes Proxy");
    assert!(
        entry.list_label().contains("com.example.exporter"),
        "the list says which plugin offered it: {}",
        entry.list_label()
    );

    let mut panel = panel;
    panel.select_preset("prores.proxy").expect("it is listed");
    let request = panel.request(&project).expect("the panel is ready");
    assert_eq!(request.preset.id, "prores.proxy");
    assert_eq!(
        request.encoder_override, None,
        "a plugin picks its own encoder"
    );
}

#[test]
fn the_open_folder_button_asks_the_host_to_reveal_the_file() {
    let (mut panel, project) = scene();
    panel.apply_event(&ExportEvent::Finished(ExportReport {
        path: PathBuf::from("/renders/sample.mp4"),
        video_frames: 240,
        audio_frames: 480,
        duration: RationalTime::from_frames(240, Rational::FPS_24),
        video_encoder: "x264enc".to_owned(),
        audio_encoder: Some("avenc_aac".to_owned()),
        muxer: "mp4mux".to_owned(),
    }));
    let mut harness = support::panel_harness_state(
        Scene {
            panel,
            project,
            asked: None,
        },
        |ui, scene| {
            if let Some(action) = scene.panel.ui(ui, &scene.project) {
                scene.asked = Some(action);
            }
        },
    );
    harness.run();
    harness
        .get_by_label(sub_ui::export_panel::OPEN_FOLDER_LABEL)
        .click();
    harness.run();
    assert_eq!(
        harness.state().asked,
        Some(ExportAction::Reveal(PathBuf::from("/renders/sample.mp4")))
    );
}
