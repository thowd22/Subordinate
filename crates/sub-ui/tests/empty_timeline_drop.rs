//! Drag imported media into the assembled editor before any tracks exist.
mod support;

use eframe::egui;
use egui_kittest::kittest::Queryable;
use sub_model::{Project, Sequence, SequenceSettings, TrackItem, TrackKind};
use sub_ui::media_bin::drag_source_id;
use sub_ui::{AppOptions, SubordinateApp};

#[test]
fn a_bin_drag_bootstraps_an_empty_timeline_and_undo_restores_it() {
    if !support::can_render() {
        return;
    }
    for existing_sequence in [false, true] {
        let fixture = support::fixture_project();
        let item = fixture
            .media
            .iter()
            .find(|item| {
                item.info.as_ref().is_some_and(|info| {
                    !info.video.is_empty() && !info.audio.is_empty() && info.duration.is_some()
                })
            })
            .expect("a probed video fixture")
            .clone();
        let media = item.id;
        let mut project = Project::new("Fresh import");
        project.root_bin.media.push(media);
        project.media.push(item);
        if existing_sequence {
            project
                .sequences
                .push(Sequence::new("Empty", SequenceSettings::default()));
        }
        let original = project.clone();
        let mut harness = support::builder::<SubordinateApp>()
            .with_size(egui::vec2(1400.0, 900.0))
            .build_eframe(|cc| {
                SubordinateApp::new(cc, AppOptions::default()).expect("editor starts")
            });
        harness.state_mut().adopt_project(project).unwrap();
        support::run_settled(&mut harness);
        let from = harness
            .ctx
            .read_response(drag_source_id(media))
            .expect("the imported item has a drag source")
            .interact_rect
            .center();
        let layout = harness
            .state_mut()
            .timeline()
            .layout()
            .expect("timeline painted");
        let to = layout.content.left_top() + egui::vec2(50.0, 24.0);
        harness.hover_at(from);
        support::run_settled(&mut harness);
        harness.drag_at(from);
        support::run_settled(&mut harness);
        harness.hover_at(to);
        support::run_settled(&mut harness);
        harness.drop_at(to);
        support::run_settled(&mut harness);
        let result = harness.state().session().project_arc();
        assert_eq!(
            result.sequences.len(),
            1,
            "the drop creates a real sequence"
        );
        assert!(
            result.sequences[0].tracks.iter().any(|track| track
                .items
                .iter()
                .any(|item| { matches!(item, TrackItem::Clip(clip) if clip.media == media) })),
            "the pointer drop must place the imported clip"
        );
        let placed = |kind| {
            result.sequences[0]
                .tracks
                .iter()
                .filter(|track| track.kind == kind)
                .flat_map(|track| track.clip_placements(result.sequences[0].settings.frame_rate))
                .find(|(clip, _)| clip.media == media)
                .expect("the real A/V pointer drop places both source streams")
        };
        let (video, video_span) = placed(TrackKind::Video);
        let (audio, audio_span) = placed(TrackKind::Audio);
        assert_ne!(video.id, audio.id);
        assert_eq!(video.source_range, audio.source_range);
        assert_eq!(video_span, audio_span, "picture and sound start in sync");
        harness.get_by_label("Edit").click();
        support::run_settled(&mut harness);
        harness.get_by_label_contains("Undo ").click();
        support::run_settled(&mut harness);
        assert_eq!(
            *harness.state().session().project_arc(),
            original,
            "one undo restores the project without removing imported media"
        );
        harness.get_by_label("Edit").click();
        support::run_settled(&mut harness);
        harness.get_by_label_contains("Redo ").click();
        support::run_settled(&mut harness);
        assert_eq!(
            *harness.state().session().project_arc(),
            *result,
            "redo restores the same sequence, track and clip identities"
        );
    }
}
