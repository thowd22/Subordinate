//! The fresh editor's context menu creates its first real timeline lane.
mod support;

use eframe::egui;
use egui_kittest::kittest::Queryable;
use sub_model::{Project, TrackKind};
use sub_ui::{AppOptions, SubordinateApp};

#[test]
fn empty_timeline_track_menus_create_a_sequence_and_undo_as_one_step() {
    if !support::can_render() {
        return;
    }
    for (label, kind) in [
        ("Add video track", TrackKind::Video),
        ("Add audio track", TrackKind::Audio),
    ] {
        let original = Project::new("Untitled");
        let mut harness = support::builder::<SubordinateApp>()
            .with_size(egui::vec2(1400.0, 900.0))
            .build_eframe(|cc| {
                SubordinateApp::new(cc, AppOptions::default()).expect("editor starts")
            });
        harness.state_mut().adopt_project(original.clone()).unwrap();
        support::run_settled(&mut harness);
        let pos = harness
            .state_mut()
            .timeline()
            .layout()
            .expect("timeline painted")
            .headers
            .left_top()
            + egui::vec2(40.0, 24.0);
        harness.event(egui::Event::PointerMoved(pos));
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::default(),
            });
            support::run_settled(&mut harness);
        }
        harness.get_by_label(label).click();
        support::run_settled(&mut harness);
        let result = harness.state().session().project_arc();
        assert_eq!(
            result.sequences.len(),
            1,
            "the menu creates a real sequence"
        );
        assert_eq!(result.sequences[0].tracks.len(), 1);
        assert_eq!(result.sequences[0].tracks[0].kind, kind);
        assert!(result.sequences[0].tracks[0].items.is_empty());
        assert!(
            harness.state_mut().timeline().header_rect(0).is_some(),
            "the created sequence is displayed"
        );
        harness.get_by_label("Edit").click();
        support::run_settled(&mut harness);
        harness.get_by_label_contains("Undo ").click();
        support::run_settled(&mut harness);
        assert_eq!(*harness.state().session().project_arc(), original);
        harness.get_by_label("Edit").click();
        support::run_settled(&mut harness);
        harness.get_by_label_contains("Redo ").click();
        support::run_settled(&mut harness);
        assert_eq!(*harness.state().session().project_arc(), *result);
    }
}
